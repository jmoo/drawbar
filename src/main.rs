use libnord::schema::Schema;
use std::io::Read;

fn main() {
    let mut stdin = std::io::stdin();

    let file = Schema::read(&mut stdin);

    println!("{:?}", file);

    // let song = electro5::Song::new(
    //     Location::from_coords(electro5::BANK_SIZE, 1, 2),
    //     Location::from_coords(electro5::BANK_SIZE, 3, 4),
    //     Location::from_coords(electro5::BANK_SIZE, 5, 6),
    //     Location::from_coords(electro5::BANK_SIZE, 7, 8),
    //     Location::from_coords(electro5::BANK_SIZE, 9, 10),
    // );
    //
    // let mut stdout = std::io::stdout();
    // let mut out = Vec::new();
    // let mut cursor = Cursor::new(&mut out);
    // song.to_stream(&mut cursor).unwrap();
    // stdout.write_all(&out).unwrap();
}
