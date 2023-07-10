pub trait FromBytes<T> {
    const BITS: usize;
    const BYTES: usize;

    fn from_bytes(bytes: &[u8]) -> Result<T, std::io::Error>;
}

pub trait FromReader<T: FromBytes<T>> {
    fn from_reader(reader: &mut impl std::io::Read) -> Result<T, std::io::Error>;
}
