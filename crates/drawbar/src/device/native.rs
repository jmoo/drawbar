//! Desktop link: `nusb` enumeration, and a thread that owns the transport.
//!
//! The worker is a plain thread blocking on its command channel. Nothing about the
//! protocol is concurrent — one transaction at a time — so a thread that runs one
//! command to completion and then waits is the whole scheduler.

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use nord_usb::device::Device;
use nord_usb::transport::{usb, UsbTransport, CLASS_VENDOR_SPECIFIC};

use super::worker::{self, Emit, Flow};
use super::{DeviceCard, DeviceCmd, DeviceEvent};

/// How often [`Link::join`] looks at a worker it is waiting for.
const SETTLE: Duration = Duration::from_millis(10);

pub struct Link {
    ctx: egui::Context,
    events: Sender<DeviceEvent>,
    /// `None` while disconnected. Dropping it is what ends the worker thread.
    commands: Option<Sender<DeviceCmd>>,
    /// The running worker, kept so the way out can wait for the session it is inside.
    worker: Option<JoinHandle<()>>,
}

impl Link {
    pub fn new(ctx: egui::Context, events: Sender<DeviceEvent>) -> Link {
        Link {
            ctx,
            events,
            commands: None,
            worker: None,
        }
    }

    pub fn connect(&mut self) {
        let (tx, rx) = mpsc::channel::<DeviceCmd>();
        self.commands = Some(tx);
        let emit = Emit::new(self.events.clone(), self.ctx.clone());

        self.worker = Some(std::thread::spawn(move || {
            let mut device = match open() {
                Ok((card, device)) => {
                    emit.send(DeviceEvent::Connected(card));
                    device
                }
                Err(why) => {
                    emit.send(DeviceEvent::ConnectFailed(why));
                    return;
                }
            };
            let mut flow = nord_usb::block_on(worker::announce(&mut device, &emit));
            // `recv` ends when the UI drops its sender, so a disconnect that races the
            // thread still stops it.
            if flow == Flow::Continue {
                flow = Flow::Released;
                while let Ok(cmd) = rx.recv() {
                    flow = nord_usb::block_on(worker::run(&mut device, cmd, &emit));
                    if flow != Flow::Continue {
                        break;
                    }
                }
            }
            // Dropping the device drops its transport, releasing the claimed interface,
            // which is what lets Nord Sound Manager and nord-cli have it back.
            drop(device);
            emit.send(DeviceEvent::Disconnected {
                lost: flow == Flow::Lost,
            });
        }));
    }

    pub fn disconnect(&mut self) {
        // Sent, then the sender is dropped: the queued command still arrives, and the
        // closed channel ends the loop even if it does not.
        if let Some(tx) = self.commands.take() {
            let _ = tx.send(DeviceCmd::Disconnect);
        }
    }

    /// Wait for the worker to finish what it is doing, up to `wait`.
    ///
    /// ⚠️ Bounded, and the handle is dropped either way: an instrument that has stopped
    /// answering must not hold the window open. The session is closed by the worker
    /// itself, so this waits for it rather than doing anything to the transport.
    pub fn join(&mut self, wait: Duration) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let since = Instant::now();
        while !worker.is_finished() && since.elapsed() < wait {
            std::thread::sleep(SETTLE);
        }
        if worker.is_finished() {
            let _ = worker.join();
        }
    }

    pub fn send(&mut self, cmd: DeviceCmd) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(cmd);
        }
    }
}

/// The first attached Clavia, with the descriptor facts the card shows.
///
/// The vendor-interface search is [`UsbTransport::open`]'s own, run again here for the
/// interface number the card reports; the transport is what claims it.
fn open() -> Result<(DeviceCard, Device<UsbTransport>), String> {
    let devices = usb::list().map_err(|e| e.to_string())?;
    let info = devices
        .into_iter()
        .next()
        .ok_or("no Clavia device found — is the instrument awake and on a data cable?")?;

    let Some(interface) = info
        .interfaces()
        .find(|i| i.class() == CLASS_VENDOR_SPECIFIC)
    else {
        return Err(format!(
            "{} exposes no vendor interface; this tool cannot drive it",
            info.product_string().unwrap_or("the attached device"),
        ));
    };
    let interface = interface.interface_number();

    let transport = UsbTransport::open(&info).map_err(|e| e.to_string())?;
    // Endpoint 0 is outside sessions; failure here hides identity details but does not
    // make the bulk transport unusable.
    let identity = transport.identity().ok();

    let card = DeviceCard {
        build: identity.map(|id| id.build),
        firmware: identity.map(|id| id.firmware),
        interface: Some(interface),
        kind: identity.map(|id| id.kind),
        manufacturer: info.manufacturer_string().map(str::to_string),
        max_transfer: identity.map(|id| id.max_transfer),
        product: info
            .product_string()
            .unwrap_or("unnamed device")
            .to_string(),
        product_id: info.product_id(),
        serial: info.serial_number().map(str::to_string),
        vendor_id: info.vendor_id(),
    };
    let device = Device::new(transport);
    Ok((card, device))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠️ The exit waits for the worker to close its session, but only so long: an
    /// instrument that has stopped answering must not hold the window open.
    #[test]
    fn waiting_for_the_worker_is_bounded() {
        let mut link = Link::new(egui::Context::default(), mpsc::channel().0);
        let (stop, held) = mpsc::channel::<()>();
        link.worker = Some(std::thread::spawn(move || {
            let _ = held.recv();
        }));

        let wait = Duration::from_millis(50);
        let started = Instant::now();
        link.join(wait);
        assert!(started.elapsed() < wait * 10, "{:?}", started.elapsed());
        assert!(link.worker.is_none(), "and the handle is let go either way");
        drop(stop);
    }
}
