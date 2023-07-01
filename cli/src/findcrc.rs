use std::io::Read;

use crcxx::crc32::{catalog::CRC_32_ISO_HDLC, *};

const CRC_32_SLICES: usize = 16;
const CRC_32: Crc<LookupTable256xN<CRC_32_SLICES>> =
    Crc::<LookupTable256xN<CRC_32_SLICES>>::new(&CRC_32_ISO_HDLC);

fn main() {
    let mut stdin = std::io::stdin();

    // const CRC_START: usize = 0x18;
    // const CRC_SIZE: usize = 4;

    let mut out: Vec<u8> = Vec::new();
    stdin.read_to_end(&mut out).unwrap();

    let mut extract: Vec<u8> = Vec::new();
    out[0x18..0x1c].clone_into(&mut extract);
    let real_le = u32::from_le_bytes(extract.as_slice().try_into().unwrap());
    let real_be = u32::from_be_bytes(extract.as_slice().try_into().unwrap());

    for i in 0..(out.len() - 1) {
        for j in 0..(out.len() - i + 1) {
            let checksum = CRC_32.compute(&out[i..(j + i)]);

            if (checksum == real_le) || (checksum == real_be) {
                println!("{:0x?}-{:0x?}: {:0x?}", i, i + j, checksum);
            } //else {
              //  println!("{:0x}-{:0x}: {:0x?} ({:0x?}:{:0x?})", i, j+i, checksum, real_le, real_be);
              // }
        }
    }
}
