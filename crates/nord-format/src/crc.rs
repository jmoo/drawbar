use crcxx::crc32::{catalog::CRC_32_ISO_HDLC, *};

use crate::error::Error;
use std::io;
use std::io::{Read, Seek, SeekFrom, Write};

const CRC_32_SLICES: usize = 16;
const CRC_32: Crc<LookupTable256xN<CRC_32_SLICES>> =
    Crc::<LookupTable256xN<CRC_32_SLICES>>::new(&CRC_32_ISO_HDLC);

pub struct MultipartCrc32<'a> {
    accumulator: ComputeMultipart<'a, LookupTable256xN<CRC_32_SLICES>>,
    buffer: Vec<u8>,
    pub first_byte: u64,
    pub length: u64,
}

impl<'a> MultipartCrc32<'a> {
    pub fn new(first_byte: u64, length: u64) -> MultipartCrc32<'a> {
        MultipartCrc32 {
            accumulator: CRC_32.compute_multipart(),
            buffer: Vec::new(),
            first_byte,
            length,
        }
    }

    pub fn update(&mut self, pos: u64, bytes: &[u8]) {
        let mut offset: u64 = 0;

        for byte in bytes {
            let current_pos = pos + offset;

            if current_pos < self.first_byte || (current_pos > (self.first_byte + self.length)) {
                offset += 1;
                continue;
            }

            self.buffer.push(*byte);
            offset += 1;
        }

        if !self.buffer.is_empty() {
            self.accumulator.update(self.buffer.as_slice());
            self.buffer.clear();
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

        match self.inner.read(buf) {
            Ok(size) => {
                self.calc.update(pos, buf);
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
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::Other,
                    "Attempted to calculate multiple checksums with single instance",
                )));
            }

            self.buffer_writes = true;
            self.buffer_pos = self.inner.stream_position()?;
            self.skip_bytes = 4;
            return Ok(0xFFFFFFFF);
        }

        // We are in the middle of calculating the checksum so we cannot return a result yet
        Err(Error::Io(io::Error::new(
            io::ErrorKind::Other,
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
            return match self.inner.write(self.buffer.as_slice()) {
                Ok(_size) => {
                    self.buffer.clear();
                    Ok(buf.len())
                }
                Err(e) => Err(e),
            };
        }

        let pos = self.inner.stream_position()?;

        match self.inner.write(buf) {
            Ok(size) => {
                self.calc.update(pos, buf);
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
