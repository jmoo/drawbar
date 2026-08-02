use crcxx::crc32::{catalog::CRC_32_ISO_HDLC, *};

use crate::error::Error;
use std::io;
use std::io::{Read, Seek, SeekFrom, Write};

const CRC_32_SLICES: usize = 16;
const CRC_32: Crc<LookupTable256xN<CRC_32_SLICES>> =
    Crc::<LookupTable256xN<CRC_32_SLICES>>::new(&CRC_32_ISO_HDLC);

/// Accumulates the CRC of bytes falling inside one region of a stream.
///
/// ⚠️ `length` is the region's **last byte index minus `first_byte`** — an inclusive
/// span, so the region covers `length + 1` bytes. Every `Schema` passes
/// `last_byte - 0x2c`; passing an end-exclusive bound instead reads one stray byte and,
/// worse, stops `CrcWriter` from ever flushing a region that ends at EOF.
pub struct MultipartCrc32<'a> {
    accumulator: ComputeMultipart<'a, LookupTable256xN<CRC_32_SLICES>>,
    pub first_byte: u64,
    pub length: u64,
}

impl<'a> MultipartCrc32<'a> {
    pub fn new(first_byte: u64, length: u64) -> MultipartCrc32<'a> {
        MultipartCrc32 {
            accumulator: CRC_32.compute_multipart(),
            first_byte,
            length,
        }
    }

    pub fn update(&mut self, pos: u64, bytes: &[u8]) {
        // The bytes are contiguous from `pos`, so the part inside the region is a
        // single slice: the overlap of `pos..pos + len` with the inclusive span.
        let end = pos + bytes.len() as u64;
        let lo = self.first_byte.max(pos);
        let hi = (self.first_byte + self.length + 1).min(end);
        if lo < hi {
            self.accumulator
                .update(&bytes[(lo - pos) as usize..(hi - pos) as usize]);
        }
    }

    pub fn checksum(&self) -> u32 {
        self.accumulator.value()
    }
}

pub struct CrcReader<'a, R: Read + Seek> {
    calc: MultipartCrc32<'a>,
    inner: &'a mut R,
}

impl<'a, R: Read + Seek> CrcReader<'a, R> {
    pub fn new(first_byte: u64, length: u64) -> impl Fn(&'a mut R) -> CrcReader<'a, R> {
        move |inner: &'a mut R| -> CrcReader<'a, R> {
            CrcReader {
                calc: MultipartCrc32::new(first_byte, length),
                inner,
            }
        }
    }

    pub fn checksum(&self) -> u32 {
        self.calc.checksum()
    }
}

impl<'a, W: Read + Seek> Read for CrcReader<'a, W> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let pos = self.inner.stream_position()?;

        // Only the bytes actually read — `buf` past `size` is stale garbage that a
        // short read must not hash.
        match self.inner.read(buf) {
            Ok(size) => {
                self.calc.update(pos, &buf[..size]);
                Ok(size)
            }
            Err(e) => Err(e),
        }
    }
}

impl<'a, W: Read + Seek> Seek for CrcReader<'a, W> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

pub struct CrcWriter<'a, W: Write + Seek> {
    calc: MultipartCrc32<'a>,
    inner: &'a mut W,
    buffer: Vec<u8>,
    buffer_pos: u64,
    buffer_writes: bool,
    skip_bytes: u64,
}

impl<'a, W: Write + Seek> CrcWriter<'a, W> {
    pub fn new(first_byte: u64, length: u64) -> impl Fn(&'a mut W) -> CrcWriter<'a, W> {
        move |inner: &'a mut W| -> CrcWriter<'a, W> {
            CrcWriter {
                calc: MultipartCrc32::new(first_byte, length),
                buffer: Vec::new(),
                inner,
                buffer_writes: false,
                buffer_pos: 0,
                skip_bytes: 0,
            }
        }
    }

    // If the checksum has already been calculated then return the result, otherwise buffer writes and calculate
    pub fn checksum(&mut self) -> Result<u32, Error> {
        let pos = self.inner.stream_position()?;

        // Checksum should already be calculated at this point
        if pos > self.calc.first_byte && pos > (self.calc.first_byte + self.calc.length) {
            return Ok(self.calc.checksum());
        }

        // Checksum has not been calculated yet so we need to buffer writes and calculate it
        if pos < self.calc.first_byte {
            // checksum() called before calculation completed
            if self.buffer_writes {
                return Err(Error::Io(io::Error::other(
                    "Attempted to calculate multiple checksums with single instance",
                )));
            }

            self.buffer_writes = true;
            self.buffer_pos = self.inner.stream_position()?;
            self.skip_bytes = 4;
            return Ok(0xFFFFFFFF);
        }

        // We are in the middle of calculating the checksum so we cannot return a result yet
        Err(Error::Io(io::Error::other(
            "Attempted to calculate checksum in the middle of the buffer",
        )))
    }
}

impl<'a, W: Write + Seek> Write for CrcWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.buffer_writes {
            let mut offset: u64 = 0;

            for byte in buf {
                if self.skip_bytes > 0 {
                    offset += 1;
                    self.skip_bytes -= 1;
                    continue;
                }

                self.buffer.push(*byte);
                offset += 1;
            }

            self.calc.update(self.buffer_pos, buf);
            self.buffer_pos += offset;

            if self.buffer_pos > (self.calc.first_byte + self.calc.length) {
                self.buffer_writes = false;

                self.inner.write_all(&self.calc.checksum().to_le_bytes())?;
            } else {
                return Ok(buf.len());
            }
        }

        if !self.buffer.is_empty() {
            // `write_all`, not `write`: a short write here would silently drop the tail
            // of the buffered body while this call reports full success.
            self.inner.write_all(self.buffer.as_slice())?;
            self.buffer.clear();
            return Ok(buf.len());
        }

        let pos = self.inner.stream_position()?;

        // Only the bytes accepted — hashing past `size` would checksum bytes the
        // caller will retry, counting them twice.
        match self.inner.write(buf) {
            Ok(size) => {
                self.calc.update(pos, &buf[..size]);
                Ok(size)
            }
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<'a, W: Write + Seek> Seek for CrcWriter<'a, W> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A reader that returns one byte per call — the legal short reads `Read` permits.
    struct OneByte<R>(R);

    impl<R: Read> Read for OneByte<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let take = 1.min(buf.len());
            self.0.read(&mut buf[..take])
        }
    }

    impl<R: Seek> Seek for OneByte<R> {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.0.seek(pos)
        }
    }

    /// A short read hashes only the bytes it returned — the stale tail of the caller's
    /// buffer must never reach the accumulator.
    #[test]
    fn a_short_read_hashes_only_what_was_read() {
        let data = [0x11u8, 0x22, 0x33, 0x44];

        let mut whole = Cursor::new(data.to_vec());
        let mut r = CrcReader::new(0, 3)(&mut whole);
        let mut buf = [0u8; 4];
        r.read_exact(&mut buf).unwrap();
        let expected = r.checksum();

        let mut trickle = OneByte(Cursor::new(data.to_vec()));
        let mut r = CrcReader::new(0, 3)(&mut trickle);
        // A large, dirty buffer: the bytes past each one-byte read are garbage.
        let mut buf = [0xffu8; 4];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(buf, data);
        assert_eq!(r.checksum(), expected, "short reads changed the checksum");
    }

    /// ⚠️ The second argument to `new` is the *last byte index* of the region minus the
    /// first — an inclusive span, not a byte count. `ne5s` passed the end-exclusive
    /// form and its writer never flushed; this pins the convention.
    ///
    /// The layout mirrors every `Schema` writer: the checksum field precedes the body,
    /// so `checksum()` is called ahead of the region and writes buffer until the region
    /// completes. The region here ends at the last byte of the stream — nothing follows
    /// to push the position past it — so the flush must fire when the region is
    /// complete, not only once something is written beyond it.
    #[test]
    fn a_region_ending_at_eof_still_flushes() {
        // 8 bytes ahead, crc at 8..12, body at 12..=15 — the file ends with the region.
        let (first, last) = (12u64, 15u64);
        let mut inner = Cursor::new(Vec::new());
        {
            let mut w = CrcWriter::new(first, last - first)(&mut inner);
            w.write_all(&[0u8; 8]).unwrap();
            let placeholder = w.checksum().unwrap();
            assert_eq!(placeholder, 0xFFFFFFFF, "not yet computable: a placeholder");
            w.write_all(&placeholder.to_le_bytes()).unwrap();
            w.write_all(&[0xaa, 0xbb, 0xcc, 0xdd]).unwrap();
        }

        let out = inner.into_inner();
        assert_eq!(out.len(), 16, "the buffered body was never flushed");
        assert_eq!(&out[12..], &[0xaa, 0xbb, 0xcc, 0xdd]);

        // The checksum written at 8..12 is the checksum of the region.
        let mut crc = MultipartCrc32::new(first, last - first);
        crc.update(0, &out);
        assert_eq!(&out[8..12], &crc.checksum().to_le_bytes());
    }
}
