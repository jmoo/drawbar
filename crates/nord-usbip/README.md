# nord-usbip

A **USB/IP server** around [`nord-emu`](../nord-emu): the emulated Clavia / Nord
instrument on a real USB stack.

`nord-emu` models the device side of the vendor protocol in-process, which reaches
everything built on `nord_usb::Transport` — and nothing else. This crate serves the
same model over the [USB/IP protocol](https://docs.kernel.org/usb/usbip_protocol.html),
so an **unmodified host** attaches it as an actual USB device: it enumerates with
Clavia's vendor id, the Electro 5 product id, and the vendor-specific interface every
`nord-usb` backend looks for, and the vendor protocol runs over its bulk endpoints
exactly as a cable would carry it.

```sh
# serve (any machine)
cargo run -p nord-usbip -- --port 3240 --load 2:14=some-program.ne5p

# attach (Linux)
sudo modprobe vhci-hcd
usbip list -r <server>
sudo usbip attach -r <server> -b 1-1

# now the emulated instrument is a USB device on this machine:
nord device info
nord list
nord put some-program.ne5p 2:15

# detach (state survives — a detach is a cable pull, not a power cycle)
sudo usbip detach -p 0
```

On Windows, [usbip-win](https://github.com/vadimgrn/usbip-win2) attaches the same
server — which is the road to running real Nord Sound Manager against the emulator.

## What the bus adds

Two behaviors the in-process transport cannot express run here the way hardware runs
them:

- a **silent device** leaves the host's IN transfer pending until the host's own
  timeout cancels it (`CMD_UNLINK`), instead of returning an error;
- a **stalled instrument** (see `EmuDevice::stall_endpoints`) hangs bulk transfers in
  both directions while endpoint 0 — descriptors and the identity words — keeps
  answering, which is exactly the state `nord device info` still works in on
  hardware.

## Fidelity notes

- The identity **shapes** (which endpoint-0 vendor requests exist, their widths, that
  an unrecognised request stalls) are confirmed on hardware; the default **values**
  (`firmware`, `build`, `kind`, `max_transfer`) are placeholders — set them from a
  real instrument (`nord device info` prints all four) via `GadgetConfig`.
- The configuration descriptor exposes **only the vendor interface**. The real
  instrument also carries a USB-MIDI interface, omitted so no host MIDI driver binds
  to an emulator with no audio behind it.
- The serial number defaults to `nord-emu`, deliberately unlike a real serial:
  anything reading it should be able to tell the instrument is emulated.
- The emulator answers instantly where hardware has real pauses (erases before large
  writes). Timing-sensitive host behavior is not exercised here.

## Testing

`cargo test -p nord-usbip` runs a scripted USB/IP peer against the server: device
list, enumeration, the identity words, a full vendor-protocol session over bulk URBs
(both directions byte-for-byte against captures), pend/unlink, and the stalled-
instrument state. No kernel module, no privileges, no network.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia", and
"Electro" are trademarks of Clavia DMI AB, used here only to identify the hardware
this protocol belongs to. The emulated device exists for interoperability testing of
this project's own software.
