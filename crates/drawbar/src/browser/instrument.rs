//! What the instrument reports about itself.

use eframe::egui;

use crate::device::Device;

/// The facts the instrument reported at connect: the USB descriptors, and the endpoint-0
/// identity the desktop transport can reach.
///
/// Read once and read-only; nothing here opens a session. Drawn under the collapsible
/// info header in [`crate::inspector`].
pub fn about(ui: &mut egui::Ui, device: &Device) {
    let Some(card) = device.state.card() else {
        return;
    };
    let mut fact = |what: &str, value: Option<String>| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(what).small().weak());
            match value {
                Some(value) => {
                    ui.label(egui::RichText::new(value).small().monospace());
                }
                None => {
                    ui.label(
                        egui::RichText::new("not asked for on this build")
                            .small()
                            .weak()
                            .italics(),
                    );
                }
            }
        });
    };
    fact("product", Some(card.product.clone()));
    fact("maker", card.manufacturer.clone());
    fact(
        "usb",
        Some(format!("{:04x}:{:04x}", card.vendor_id, card.product_id)),
    );
    fact("serial", card.serial.clone());
    fact(
        "interface",
        card.interface.map(|held| format!("{held} (vendor)")),
    );
    fact("firmware", device.state.firmware());
    fact("build", card.build.map(|held| held.to_string()));
    fact("kind", card.kind.map(|held| format!("{held:#06x}")));
    fact(
        "max transfer",
        card.max_transfer.map(|held| format!("{held} bytes")),
    );
    ui.label(
        egui::RichText::new(
            "The build and kind words are the device's answers to those requests; \
             their meaning is unknown.",
        )
        .small()
        .weak()
        .italics(),
    );
}
