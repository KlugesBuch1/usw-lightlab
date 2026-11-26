use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, TextStyle};

use crate::led::Leds;
use crate::strip;

pub struct LightLab {
    leds: Leds,
}

impl LightLab {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme(&cc.egui_ctx);
        Self { leds: Leds::new() }
    }
}
impl eframe::App for LightLab {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(13, 15, 19))
                    .inner_margin(Margin::symmetric(22, 18)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("lightlab")
                            .font(FontId::new(25.0, FontFamily::Proportional))
                            .strong()
                            .color(Color32::from_rgb(240, 242, 245)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("USW-Pro-Max-48 LED bench")
                            .size(12.0)
                            .color(Color32::from_rgb(120, 128, 140)),
                    );
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(13, 15, 19))
                    .inner_margin(Margin::symmetric(22, 6)),
            )
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(Color32::from_rgb(19, 22, 28))
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(31, 36, 44)))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("PREVIEW")
                                    .font(FontId::new(10.0, FontFamily::Monospace))
                                    .color(Color32::from_rgb(150, 158, 170)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("ports 1-48 / 24 pairs")
                                            .size(10.0)
                                            .color(Color32::from_rgb(105, 113, 125)),
                                    );
                                },
                            );
                        });
                        ui.add_space(10.0);
                        strip::show(ui, &self.leds);
                    });
            });
    }
}

fn theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(13, 15, 19);
    style.visuals.window_fill = Color32::from_rgb(19, 22, 28);
    style.visuals.extreme_bg_color = Color32::from_rgb(9, 11, 14);
    style.visuals.faint_bg_color = Color32::from_rgb(24, 28, 34);
    style.visuals.override_text_color = Some(Color32::from_rgb(219, 224, 231));
    style.visuals.selection.bg_fill = Color32::from_rgb(196, 43, 56);
    style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(255, 225, 228));
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(19, 22, 28);
    style.visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(37, 42, 50));
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(30, 34, 41);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(24, 27, 33);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(45, 51, 60));
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(39, 44, 52);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(32, 36, 43);
    style.visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(73, 80, 92));
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(196, 43, 56);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(196, 43, 56);
    style.visuals.widgets.active.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(255, 107, 118));
    style.spacing.item_spacing = egui::vec2(8.0, 9.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(11.5, FontFamily::Monospace),
        ),
    ]
    .into();
    ctx.set_style(style);
}
