//! Implementation of RFC 7845 demultiplexing of an Opus stream from an Ogg container
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use alloc::vec::Vec;
use core::{convert::TryInto, fmt};

use ogg::reading::{BasePacketReader, PageParser};

pub struct Demuxer {
    // Page decoding state
    packet_reader: BasePacketReader,
    buffer: Vec<u8>,
    page_parser: Option<PageParser>,
    got_segments: bool,
    need_bytes: usize,

    // Packet decoding state
    header: Option<Header>,
    tags: Option<ogg::Packet>,
}

impl Demuxer {
    pub fn new() -> Self {
        Self {
            packet_reader: BasePacketReader::new(),
            buffer: Vec::new(),
            page_parser: None,
            got_segments: false,
            need_bytes: HEADER_SIZE,

            header: None,
            tags: None,
        }
    }

    /// Consume a block of Ogg container data
    ///
    /// After a call to `push`, other methods may begin returning `Some`.
    pub fn push(&mut self, data: &[u8]) -> Result<()> {
        self.buffer.extend_from_slice(data);
        let mut cursor = 0;
        loop {
            let mut page_parser = match self.page_parser.take() {
                None => {
                    if cursor + HEADER_SIZE > self.buffer.len() {
                        break;
                    }
                    let (parser, segment_bytes) = PageParser::new(
                        self.buffer[cursor..cursor + HEADER_SIZE]
                            .try_into()
                            .unwrap(),
                    )
                    .map_err(|_| Error::Malformed)?;
                    self.need_bytes = segment_bytes;
                    cursor += HEADER_SIZE;
                    parser
                }
                Some(x) => x,
            };
            if !self.got_segments {
                if cursor + self.need_bytes > self.buffer.len() {
                    self.page_parser = Some(page_parser);
                    break;
                }
                let packet_data_len = page_parser
                    .parse_segments(self.buffer[cursor..cursor + self.need_bytes].to_vec());
                cursor += self.need_bytes;
                self.need_bytes = packet_data_len;
                self.got_segments = true;
            }
            if self.need_bytes > self.buffer.len() {
                self.page_parser = Some(page_parser);
                break;
            }
            self.packet_reader
                .push_page(
                    page_parser
                        .parse_packet_data(self.buffer[cursor..cursor + self.need_bytes].to_vec())
                        .map_err(|_| Error::Malformed)?,
                )
                .map_err(|_| Error::Malformed)?;
            cursor += self.need_bytes;
        }
        self.buffer.drain(..cursor);

        if self.header.is_none() {
            const ID_MAGIC: &[u8; 8] = b"OpusHead";
            loop {
                let packet = match self.packet_reader.read_packet() {
                    None => return Ok(()),
                    Some(x) => x,
                };
                if packet.first_in_stream() {
                    // Is this an Opus ogg encapsulation v1-compatible stream?
                    if !packet.data.starts_with(ID_MAGIC)
                        || packet.data.get(8).ok_or(Error::Malformed)? & 0xF0 != 0
                    {
                        continue;
                    }
                    self.header = Some(Header {
                        serial: packet.stream_serial(),
                        channels: packet.data[9],
                        pre_skip: u16::from_le_bytes(
                            packet
                                .data
                                .get(10..12)
                                .ok_or(Error::Malformed)?
                                .try_into()
                                .unwrap(),
                        ),
                        output_gain: i16::from_le_bytes(
                            packet
                                .data
                                .get(16..18)
                                .ok_or(Error::Malformed)?
                                .try_into()
                                .unwrap(),
                        ),
                    });
                    break;
                }
            }
        }

        if self.tags.is_none() {
            const COMMENT_MAGIC: &[u8; 8] = b"OpusTags";
            loop {
                let packet = match self.packet_reader.read_packet() {
                    None => return Ok(()),
                    Some(x) => x,
                };
                if Some(packet.stream_serial()) != self.header.as_ref().map(|x| x.serial) {
                    continue;
                }
                if packet.data.starts_with(COMMENT_MAGIC) {
                    self.tags = Some(packet);
                    break;
                }
            }
        }

        Ok(())
    }

    /// Access the decoded `Header`, if available
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref()
    }

    /// Extract the Opus tags, if available. Will not return `Some` before `header` does.
    pub fn tags(&self) -> Option<&[u8]> {
        self.tags.as_ref().map(|x| &x.data[..])
    }

    /// Extract a block of Opus stream data, if available. Will not return `Some` before `header` or
    /// `tags` do.
    pub fn next(&mut self) -> Option<Vec<u8>> {
        if self.header.is_none() || self.tags.is_none() {
            return None;
        }
        loop {
            let packet = self.packet_reader.read_packet()?;
            if Some(packet.stream_serial()) != self.header.as_ref().map(|x| x.serial) {
                continue;
            }
            return Some(packet.data);
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Header {
    serial: u32,
    /// Number of channels
    pub channels: u8,
    /// Number of samples to discard from the decoder output when starting playback
    pub pre_skip: u16,
    /// Encoded gain to be applied when decoding. To decode into an amplitude scaling factor,
    /// compute `10.0.powf(output_gain/(20.0*256))`.
    pub output_gain: i16,
}

const HEADER_SIZE: usize = 27;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub enum Error {
    Malformed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("malformed container")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
