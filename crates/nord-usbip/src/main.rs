//! `nord-usbip`: serve the emulated instrument over USB/IP.
//!
//! ```sh
//! nord-usbip [--port 3240] [--load BANK:SLOT=FILE ...]
//!
//! # then, on the attaching host (Linux):
//! sudo modprobe vhci-hcd
//! sudo usbip attach -r 127.0.0.1 -b 1-1
//! ```

use std::net::TcpListener;
use std::process::ExitCode;

use nord_emu::{EmuDevice, Object};
use nord_usb::wire::{Location, ObjectClass};
use nord_usbip::{handle_connection, Gadget, GadgetConfig};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nord-usbip: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut port = 3240u16;
    let mut device = EmuDevice::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                port = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--port takes a number")?;
            }
            "--load" => {
                let spec = args.next().ok_or("--load takes BANK:SLOT=FILE")?;
                load(&mut device, &spec)?;
            }
            "--help" | "-h" => {
                println!("usage: nord-usbip [--port 3240] [--load BANK:SLOT=FILE ...]");
                return Ok(());
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let mut gadget = Gadget::new(device, GadgetConfig::default());
    let listener =
        TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("bind port {port}: {e}"))?;

    eprintln!("nord-usbip: emulated Nord Electro 5 on port {port}");
    eprintln!("  attach (linux):  sudo modprobe vhci-hcd && sudo usbip attach -r <host> -b 1-1");
    eprintln!("  list:            usbip list -r <host>");

    loop {
        let (mut stream, peer) = listener.accept().map_err(|e| format!("accept: {e}"))?;
        // URBs are small and interactive; Nagle would serialize every exchange with
        // the peer's delayed ACK.
        let _ = stream.set_nodelay(true);
        eprintln!("nord-usbip: connection from {peer}");
        if let Err(e) = handle_connection(&mut stream, &mut gadget) {
            eprintln!("nord-usbip: connection ended: {e}");
        } else {
            eprintln!("nord-usbip: peer detached");
        }
    }
}

/// `--load BANK:SLOT=FILE`: put a program file into a slot before serving, so an
/// attaching host has something to browse. Panel numbering, as everywhere user-facing.
fn load(device: &mut EmuDevice, spec: &str) -> Result<(), String> {
    let (addr, path) = spec
        .split_once('=')
        .ok_or_else(|| format!("--load {spec:?}: expected BANK:SLOT=FILE"))?;
    let (bank, slot) = addr
        .split_once(':')
        .and_then(|(b, s)| Some((b.parse::<u32>().ok()?, s.parse::<u32>().ok()?)))
        .filter(|&(b, s)| b >= 1 && s >= 1)
        .ok_or_else(|| format!("--load {spec:?}: bad address {addr:?}"))?;

    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let file = nord_usb::envelope::unwrap(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "loaded".into());

    device.insert(
        ObjectClass::Program,
        Location::from_user(bank, slot),
        Object::new(&name, &file.header.tag, file.header.version, file.body.0),
    );
    Ok(())
}
