pub trait FromReader<T> {
    fn from_reader(reader: &mut impl std::io::Read) -> Result<T, std::io::Error>;
}

pub struct BitReader<'a> {
    reader: Box<&'a mut (dyn std::io::Read)>,
    offset: u8,
    buffer: u8,
}

impl<'a> std::io::Read for BitReader<'a> {
    fn read(&mut self, out: &mut [u8]) -> Result<usize, std::io::Error> {
        self.read_bits(out, 0)?;
        Ok(out.len())
    }
}

impl<'a> BitReader<'a> {
    pub fn new(reader: &'a mut impl std::io::Read) -> Self {
        Self {
            reader: Box::new(reader),
            offset: 0,
            buffer: 0,
        }
    }

    pub fn take_bits(&mut self, bits: usize) -> Result<u8, std::io::Error> {
        assert!(bits <= 8);

        let mut out = [0 as u8];
        self.read_bits(&mut out, bits % 8)?;
        Ok(out[0])
    }

    fn read_bits(&mut self, out: &mut [u8], bits: usize) -> Result<(), std::io::Error> {
        assert!(bits < 8);

        if self.offset == 0 {
            self.reader.read_exact(out)?;

            if bits == 0 {
                return Ok(());
            }
        }

        let last_index = out.len() - 1;
        let mut cap = out[last_index];

        if self.offset > 0 {
            let overflow = if bits == 0 || bits as u8 + self.offset > 8 {
                1
            } else {
                0
            };

            let mut buff = self.buffer;
            cap = buff;

            let offset_inverse = 8 - self.offset;
            let mask = 0b1111_1111 << offset_inverse;

            for i in 0..out.len() {
                buff = buff << self.offset;

                if i < out.len() - 1 + overflow {
                    let mut byte = [0 as u8];
                    self.reader.read_exact(&mut byte)?;
                    cap = byte[0];

                    buff = buff | ((mask & byte[0]) >> offset_inverse);

                    out[i] = buff;
                    buff = byte[0];
                } else {
                    out[i] = buff;
                }
            }
        }

        if bits > 0 {
            self.offset = (self.offset + bits as u8) % 8;
            let mask = 0b1111_1111 << (8 - bits);
            out[last_index] = (out[last_index] & mask) >> (8 - bits);
        }

        if self.offset > 0 {
            self.buffer = cap;
        } else {
            self.buffer = 0;
        }

        Ok(())
    }
}
