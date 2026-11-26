#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod action;
mod app;
mod hostkeys;
mod led;
mod preview;
mod remote;
mod settings;
mod ssh;
mod strip;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 820.0])
            .with_min_inner_size([820.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LightLab",
        options,
        Box::new(|cc| Ok(Box::new(app::LightLab::new(cc)))),
    )
}
