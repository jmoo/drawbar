use std::io::{Cursor, Read};
use libnord::from_stream;

fn main() {
    let mut stdin = std::io::stdin();
    let mut buffer = Vec::new();
    stdin.read_to_end(&mut buffer).unwrap();

    let mut cursor = Cursor::new(&mut buffer);
    let file = from_stream(&mut cursor);

    println!("{:?}", file);
}
