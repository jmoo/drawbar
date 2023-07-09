pub trait FromReader<T> {
    fn from_reader(reader: &mut impl std::io::Read) -> Result<T, std::io::Error>;
}

pub trait FromBytes<T> {
    fn from_bytes(bytes: &[u8]) -> Result<T, std::io::Error>;
}
