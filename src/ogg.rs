use alloc::{collections::VecDeque, vec::Vec};
use core::ops::Range;

#[derive(Default)]
pub struct Stream {
    buffer: VecDeque<u8>,
    /// Data buffered for the current packet, for each logical stream
    stream_packets: Vec<(u32, Vec<u8>)>,
    /// Position in the segment table of the current page
    segment: usize,
    /// Offset of the current packet's start
    packet_start: usize,
    /// Whether `packet` contains an incomplete prefix
    packet_continued: bool,
}

impl Stream {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn push(&mut self, data: &[u8]) {
        self.buffer.extend(data);
    }

    /// Fetch the earliest page that been read in full
    pub fn next(&mut self) -> Option<Page<'_>> {
        loop {
            let mut r = Reader {
                buffer: &self.buffer,
                cursor: 0,
            };

            // Scan until the start of a packet
            const PACKET_HEADER: &[u8; 4] = b"OggS";
            if r.get::<{ PACKET_HEADER.len() }>()? != *PACKET_HEADER {
                self.buffer.pop_front();
                continue;
            }

            let version = r.get::<1>()?[0];
            if version != 0 {
                // Unrecognized version, scan for another page
                self.buffer.drain(..PACKET_HEADER.len() + 1);
                continue;
            }

            let flags = r.get::<1>()?[0];
            let continued = flags & 0x01 != 0;
            let bos = flags & 0x02 != 0;
            let eos = flags & 0x04 != 0;
            let granule_position = u64::from_le_bytes(r.get::<8>()?);
            let stream_serial = u32::from_le_bytes(r.get::<4>()?);
            let sequence = u32::from_le_bytes(r.get::<4>()?);
            let checksum = u32::from_le_bytes(r.get::<4>()?);
            let segment_count = r.get::<1>()?[0] as usize;

            if self.segment == segment_count {
                self.segment = 0;
                self.buffer.drain(..self.packet_start);
                if eos {
                    // Free buffer for finished logical stream
                    if let Some(index) = self
                        .stream_packets
                        .iter()
                        .position(|&(s, _)| s == stream_serial)
                    {
                        self.stream_packets.swap_remove(index);
                    }
                }
                continue;
            }

            let segments_start = r.cursor;
            r.skip(segment_count)?;
            let mut segments = [0; 255];
            for (&i, o) in self
                .buffer
                .range(segments_start..segments_start + segment_count)
                .zip(&mut segments)
            {
                *o = i;
            }

            let payload_len = segments[..segment_count as usize]
                .iter()
                .copied()
                .map(usize::from)
                .sum::<usize>();
            if r.cursor.checked_add(payload_len)? > self.buffer.len() {
                return None;
            }
            if self.segment == 0 {
                self.packet_start = r.cursor;
                // Skip incomplete packets
                if let Some((_, packet)) = self
                    .stream_packets
                    .iter_mut()
                    .find(|&&mut (s, _)| s == stream_serial)
                {
                    if continued && packet.is_empty() {
                        // Tail without head
                        for &len in &segments[..segment_count] {
                            self.packet_start += len as usize;
                            self.segment += 1;
                            if len != u8::MAX {
                                break;
                            }
                        }
                    }
                    if !continued && !packet.is_empty() {
                        // Head without tail
                        packet.clear();
                    }
                }
            }

            return Some(Page {
                segment_count,
                segments,
                header: PageHeader {
                    bos,
                    eos,
                    granule_position,
                    stream_serial,
                    sequence,
                    checksum,
                },
                stream: self,
            });
        }
    }
}

pub struct Page<'a> {
    segment_count: usize,
    segments: [u8; 255],
    header: PageHeader,
    stream: &'a mut Stream,
}

impl<'a> Page<'a> {
    #[inline]
    pub fn header(&self) -> PageHeader {
        self.header
    }

    /// Read the next packet from this page
    pub fn next(&mut self) -> Option<&[u8]> {
        let i = self.next_inner()?;
        Some(&self.stream.stream_packets[i].1)
    }

    /// Read the next packet from this page and borrow it from the `Stream`
    pub fn into_next(mut self) -> Option<&'a [u8]> {
        let i = self.next_inner()?;
        Some(&self.stream.stream_packets[i].1)
    }

    fn next_inner(&mut self) -> Option<usize> {
        if self.stream.segment >= self.segment_count {
            return None;
        }

        let packet_index = match self
            .stream
            .stream_packets
            .iter()
            .position(|&(s, _)| s == self.header.stream_serial)
        {
            Some(x) => x,
            None => {
                let i = self.stream.stream_packets.len();
                self.stream
                    .stream_packets
                    .push((self.header.stream_serial, Vec::new()));
                i
            }
        };

        if !self.stream.packet_continued {
            self.stream.stream_packets[packet_index].1.clear();
        }

        // Copy out all segments from the current packet
        let mut packet_data_len = 0usize;
        let mut segment = self.stream.segment;
        self.stream.packet_continued = true;
        while segment < self.segment_count {
            let segment_len = self.segments[segment];
            segment += 1;
            packet_data_len = packet_data_len.saturating_add(segment_len as usize);
            if segment_len < 255 {
                self.stream.packet_continued = false;
                break;
            }
        }
        let packet_end = self.stream.packet_start + packet_data_len;
        fill(
            &self.stream.buffer,
            self.stream.packet_start..packet_end,
            &mut self.stream.stream_packets[packet_index].1,
        )?;

        // Set up for the next packet in the stream
        self.stream.packet_start = packet_end;
        self.stream.segment = segment;

        if !self.stream.packet_continued {
            Some(packet_index)
        } else {
            None
        }
    }
}

struct Reader<'a> {
    buffer: &'a VecDeque<u8>,
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn get<const N: usize>(&mut self) -> Option<[u8; N]> {
        if self.cursor.checked_add(N)? > self.buffer.len() {
            return None;
        }
        let mut buf = [0; N];
        for (&i, o) in self
            .buffer
            .range(self.cursor..self.cursor + N)
            .zip(buf.iter_mut())
        {
            *o = i;
        }
        self.cursor += N;
        Some(buf)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        if self.cursor.checked_add(count)? > self.buffer.len() {
            return None;
        }
        self.cursor += count;
        Some(())
    }
}

fn fill(buffer: &VecDeque<u8>, range: Range<usize>, out: &mut Vec<u8>) -> Option<()> {
    let (a, b) = buffer.as_slices();
    let split = range.end.min(a.len());
    let rest = range.end - split;
    if rest > b.len() {
        return None;
    }
    if range.start < split {
        out.extend_from_slice(&a[range.start..split]);
    }
    out.extend_from_slice(&b[range.start.saturating_sub(split)..rest]);
    Some(())
}

#[derive(Debug, Copy, Clone)]
pub struct PageHeader {
    /// Whether this is the first packet of a logical bitstream
    pub bos: bool,
    /// Whether this is the last packet of a logical bitstream
    pub eos: bool,
    pub granule_position: u64,
    pub stream_serial: u32,
    pub sequence: u32,
    pub checksum: u32,
}
