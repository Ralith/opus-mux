//! Implementation of RFC 7845 demultiplexing of an Opus stream from an Ogg container
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod ogg;

use alloc::boxed::Box;
use core::{convert::TryInto, fmt};

pub struct Demuxer {
    // Page decoding state
    stream: ogg::Stream,

    // Packet decoding state
    header: Option<InternalHeader>,
    tags: Option<Box<[u8]>>,
}

impl Demuxer {
    pub fn new() -> Self {
        Self {
            stream: ogg::Stream::new(),

            header: None,
            tags: None,
        }
    }

    /// Consume a block of Ogg container data
    ///
    /// After a call to `push`, other methods may begin returning `Some`.
    pub fn push(&mut self, data: &[u8]) -> Result<()> {
        self.stream.push(data);
        'next_page: loop {
            let mut page = match self.stream.next() {
                None => {
                    break;
                }
                Some(x) => x,
            };
            let page_header = page.header();

            if self.header.is_none() {
                loop {
                    let packet = match page.next() {
                        None => continue 'next_page,
                        Some(x) => x,
                    };

                    // Read packets until we get the header or run out
                    const ID_MAGIC: &[u8; 8] = b"OpusHead";
                    if page_header.bos {
                        // Is this an Opus ogg encapsulation v1-compatible stream?
                        if !packet.starts_with(ID_MAGIC)
                            || packet.get(8).ok_or(Error::Malformed)? & 0xF0 != 0
                        {
                            continue;
                        }
                        self.header = Some(InternalHeader {
                            serial: page_header.stream_serial,
                            header: Header {
                                channels: packet[9],
                                pre_skip: u16::from_le_bytes(
                                    packet
                                        .get(10..12)
                                        .ok_or(Error::Malformed)?
                                        .try_into()
                                        .unwrap(),
                                ),
                                output_gain: i16::from_le_bytes(
                                    packet
                                        .get(16..18)
                                        .ok_or(Error::Malformed)?
                                        .try_into()
                                        .unwrap(),
                                ),
                            },
                        });
                        break;
                    }
                }
            }

            if self.tags.is_none() {
                loop {
                    let packet = match page.next() {
                        None => continue 'next_page,
                        Some(x) => x,
                    };

                    // Read packets until we get the tags or run out
                    const COMMENT_MAGIC: &[u8; 8] = b"OpusTags";
                    if Some(page_header.stream_serial) != self.header.as_ref().map(|x| x.serial) {
                        continue;
                    }
                    if packet.starts_with(COMMENT_MAGIC) {
                        self.tags = Some(packet.into());
                        break;
                    }
                }
            }

            break;
        }

        Ok(())
    }

    /// Access the decoded `Header`, if available
    #[inline]
    pub fn header(&self) -> Option<&Header> {
        self.header.as_ref().map(|x| &x.header)
    }

    /// Extract the Opus tags, if available. Will not return `Some` before `header` does.
    #[inline]
    pub fn tags(&self) -> Option<&[u8]> {
        self.tags.as_deref()
    }

    /// Extract a block of Opus stream data, if available. Will not return `Some` before `header` or
    /// `tags` do.
    pub fn next(&mut self) -> Option<&[u8]> {
        let header = match (&self.header, &self.tags) {
            (&Some(ref h), &Some(_)) => h,
            _ => return None,
        };
        loop {
            let page = self.stream.next()?;
            if page.header().stream_serial != header.serial {
                continue;
            }
            if let Some(packet) = page.into_next() {
                // Hack around bad lifetime check: https://github.com/rust-lang/rust/issues/54663
                unsafe {
                    return Some(core::mem::transmute::<&[u8], &[u8]>(packet));
                }
            }
        }
    }
}

impl Default for Demuxer {
    fn default() -> Self {
        Self::new()
    }
}

struct InternalHeader {
    serial: u32,
    header: Header,
}

#[derive(Debug, Copy, Clone)]
pub struct Header {
    /// Number of channels
    pub channels: u8,
    /// Number of samples to discard from the decoder output when starting playback
    pub pre_skip: u16,
    /// Encoded gain to be applied when decoding. To decode into an amplitude scaling factor,
    /// compute `10.0.powf(f32::from(output_gain)/(20.0*256.0))`.
    pub output_gain: i16,
}

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
