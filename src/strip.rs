use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::led::{to_u8, Leds, PORT_COUNT};

pub fn show(ui: &mut egui::Ui, leds: &Leds) -> egui::Response {
    let width = ui.available_width().max(240.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 104.0), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, CornerRadius::same(14), Color32::from_rgb(12, 14, 18));
    painter.rect_stroke(
        rect,
        CornerRadius::same(14),
        Stroke::new(1.0_f32, Color32::from_rgb(28, 32, 39)),
        StrokeKind::Inside,
    );

    let inner = rect.shrink2(Vec2::new(14.0, 15.0));
    let pair_gap = 6.0;
    let led_gap = 2.0;
    let pairs = PORT_COUNT / 2;
    let total_gaps = pair_gap * (pairs as f32 - 1.0) + led_gap * pairs as f32;
    let led_w = ((inner.width() - total_gaps) / PORT_COUNT as f32).clamp(3.0, 18.0);
    let led_h = (inner.height() - 12.0).clamp(28.0, 56.0);
    let pair_w = led_w * 2.0 + led_gap;
    let strip_w = pair_w * pairs as f32 + pair_gap * (pairs as f32 - 1.0);
    let start_x = inner.center().x - strip_w / 2.0;
    let led_y = inner.center().y - led_h / 2.0;
    let radius = CornerRadius::same(led_w.min(led_h).mul_add(0.35, 1.0) as u8);

    for pair in 0..pairs {
        let x = start_x + pair as f32 * (pair_w + pair_gap);
        let slot = Rect::from_min_size(
            Pos2::new(x - 3.0, led_y - 6.0),
            Vec2::new(pair_w + 6.0, led_h + 12.0),
        );
        painter.rect_filled(slot, CornerRadius::same(8), Color32::from_rgb(17, 20, 25));
        painter.rect_stroke(
            slot,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, Color32::from_rgb(25, 29, 36)),
            StrokeKind::Inside,
        );

        for offset in 0..2 {
            let led_rect = Rect::from_min_size(
                Pos2::new(x + offset as f32 * (led_w + led_gap), led_y),
                Vec2::new(led_w, led_h),
            );
            let port = pair * 2 + offset;
            let values = leds.port(port + 1);
            let rgb = [to_u8(values[0]), to_u8(values[1]), to_u8(values[2])];
            let peak = rgb.iter().copied().max().unwrap_or(0);

            if peak > 0 {
                let glow_alpha = (peak as f32 * 0.35).clamp(18.0, 96.0) as u8;
                painter.rect_filled(
                    led_rect.expand(3.0),
                    radius,
                    Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], glow_alpha / 3),
                );
                painter.rect_filled(
                    led_rect.expand(1.5),
                    radius,
                    Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], glow_alpha),
                );
            }

            let body = if peak == 0 {
                Color32::from_rgb(25, 29, 35)
            } else {
                Color32::from_rgb(rgb[0], rgb[1], rgb[2])
            };
            painter.rect_filled(led_rect, radius, body);
            painter.rect_stroke(
                led_rect,
                radius,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 18)),
                StrokeKind::Inside,
            );

            let gloss = Rect::from_min_size(
                led_rect.min + Vec2::new(2.0, 2.0),
                Vec2::new(
                    (led_rect.width() - 4.0).max(0.0),
                    (led_rect.height() * 0.34).max(0.0),
                ),
            );
            painter.rect_filled(
                gloss,
                CornerRadius::same(2),
                Color32::from_rgba_unmultiplied(255, 255, 255, if peak > 0 { 28 } else { 10 }),
            );
        }
    }

    response
}
