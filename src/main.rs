#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod led;
mod strip;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 780.0])
            .with_min_inner_size([780.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LightLab",
        options,
        Box::new(|cc| Ok(Box::new(app::LightLab::new(cc)))),
    )
}
