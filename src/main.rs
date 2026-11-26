#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

struct LightLab;

impl eframe::App for LightLab {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("lightlab");
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 760.0])
            .with_min_inner_size([760.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LightLab",
        options,
        Box::new(|_cc| Ok(Box::new(LightLab))),
    )
}
