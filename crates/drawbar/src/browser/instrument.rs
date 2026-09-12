//! What the instrument says about itself.

use eframe::egui;

use crate::device::Device;

/// What the instrument said about itself, for the times that is the question.
///
/// Read-only and asked for once, at connect: the descriptors, and the endpoint-0
/// identity the desktop transport can reach. Nothing here opens a session. The INFO
/// panel in [`crate::inspector`] is where it is collapsed and kept.
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
            "The build and kind words are what the device answers at their \
             requests; what they mean is not pinned down.",
        )
        .small()
        .weak()
        .italics(),
    );
}
