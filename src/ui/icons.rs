// English comments: font-independent vector icons.
// Every glyph here is painted with lines (no font codepoints), so buttons
// never render as tofu boxes on machines without symbol fonts.

/// Small toolbar icons painted with the egui painter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    X,
    Plus,
}

/// Line segments (pairs of offsets from center) for an icon at radius `r`.
/// Pure geometry: unit-tested, no egui context needed.
pub fn segments(icon: Icon, r: f32) -> Vec<[[f32; 2]; 2]> {
    match icon {
        Icon::ChevronLeft => vec![
            [[r * 0.4, -r], [-r * 0.6, 0.0]],
            [[-r * 0.6, 0.0], [r * 0.4, r]],
        ],
        Icon::ChevronRight => vec![
            [[-r * 0.4, -r], [r * 0.6, 0.0]],
            [[r * 0.6, 0.0], [-r * 0.4, r]],
        ],
        Icon::ChevronUp => vec![
            [[-r, r * 0.4], [0.0, -r * 0.6]],
            [[0.0, -r * 0.6], [r, r * 0.4]],
        ],
        Icon::ChevronDown => vec![
            [[-r, -r * 0.4], [0.0, r * 0.6]],
            [[0.0, r * 0.6], [r, -r * 0.4]],
        ],
        Icon::X => vec![
            [[-r * 0.7, -r * 0.7], [r * 0.7, r * 0.7]],
            [[r * 0.7, -r * 0.7], [-r * 0.7, r * 0.7]],
        ],
        Icon::Plus => vec![
            [[-r * 0.8, 0.0], [r * 0.8, 0.0]],
            [[0.0, -r * 0.8], [0.0, r * 0.8]],
        ],
    }
}

/// Clickable icon button (~22px square) with hover/pressed feedback and tooltip.
/// Never touches fonts: safe on any OS without extra symbol fonts.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> egui::Response {
    let size = egui::vec2(24.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(
            rect,
            4.0,
            ui.visuals().widgets.hovered.bg_fill,
        );
    }
    if resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            4.0,
            ui.visuals().widgets.active.bg_fill,
        );
    }
    let fg = ui.style().interact(&resp).fg_stroke.color;
    let stroke = egui::Stroke::new(2.0_f32, fg);
    let c = rect.center();
    for [[x1, y1], [x2, y2]] in segments(icon, 5.0) {
        ui.painter().line_segment(
            [
                egui::pos2(c.x + x1, c.y + y1),
                egui::pos2(c.x + x2, c.y + y2),
            ],
            stroke,
        );
    }
    resp = resp.on_hover_text(tooltip);
    resp
}

/// Dropdown menu button: text + painted chevron-down, zero font glyphs
/// (never tofu — same philosophy as `icon_button`). Behavior is exactly
/// `ui.menu_button` (same open/hover/close); the chevron is decoration
/// painted inside the button's right padding, which two trailing NBSPs
/// reserve (U+00A0 is Latin-1 so every font has it, and unlike a regular
/// space it is never trimmed from measurement).
pub fn menu_drop_down<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let padded = format!("{title}\u{a0}\u{a0}");
    let resp = ui.menu_button(padded, add_contents);
    let rect = resp.response.rect;
    let c = egui::pos2(rect.right() - 7.5, rect.center().y);
    let fg = ui.style().interact(&resp.response).fg_stroke.color;
    let stroke = egui::Stroke::new(2.0_f32, fg);
    for [[x1, y1], [x2, y2]] in segments(Icon::ChevronDown, 2.8) {
        ui.painter().line_segment(
            [
                egui::pos2(c.x + x1, c.y + y1),
                egui::pos2(c.x + x2, c.y + y2),
            ],
            stroke,
        );
    }
    resp
}

/// App logo (assets/asislog-32.png, baked into the binary).
/// `None` when the bytes fail to decode: callers must paint
/// `paint_logo_fallback` instead (vector A, still no fonts/symbols).
pub const LOGO_PNG: &[u8] = include_bytes!("../../assets/asislog-32.png");

/// Decoded logo image for the header, or None (use the vector fallback).
pub fn logo_image() -> Option<egui::Image<'static>> {
    if image::load_from_memory(LOGO_PNG).is_ok() {
        Some(egui::Image::from_bytes(
            "bytes://asislog-logo-32.png",
            LOGO_PNG,
        ))
    } else {
        None
    }
}

/// Segmen garis 2D: pasangan titik [x, y] relatif terhadap pusat logo.
pub type LogoSegments = Vec<[[f32; 2]; 2]>;

/// Logo geometry in a `size`×`size` box, fractions of size:
/// (leg segments, teal bar segments). Pure: unit-tested.
pub fn logo_shapes(size: f32) -> (LogoSegments, LogoSegments) {
    let p = |x: f32, y: f32| [x * size, y * size];
    // Bold A legs (mirror the SVG master proportions).
    let legs = vec![
        [p(0.297, 0.805), p(0.484, 0.227)],
        [p(0.703, 0.805), p(0.516, 0.227)],
    ];
    // Teal crossbar + two tapering flow lines.
    let bars = vec![
        [p(0.336, 0.453), p(0.664, 0.453)],
        [p(0.172, 0.555), p(0.828, 0.555)],
        [p(0.234, 0.672), p(0.766, 0.672)],
    ];
    (legs, bars)
}

/// Primary vector logo: dark rounded tile + white A + teal bars.
/// Zero-allocation, sharp at any DPI, never triggers broken image fallback.
pub fn paint_logo(ui: &mut egui::Ui, size: f32) {
    paint_logo_fallback(ui, size);
}

/// Vector fallback logo: dark rounded tile + white A + teal bars.
/// Used only when the baked PNG cannot be decoded.
pub fn paint_logo_fallback(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        size * 0.227, // rx 58/256 like the master
        egui::Color32::from_rgb(0x0D, 0x16, 0x22),
    );
    let origin = rect.min;
    let at = |p: [f32; 2]| egui::pos2(origin.x + p[0], origin.y + p[1]);
    let (legs, bars) = logo_shapes(size);
    for [a, b] in legs {
        painter.line_segment(
            [at(a), at(b)],
            egui::Stroke::new((size * 0.121).max(2.0), egui::Color32::from_rgb(0xF4, 0xF7, 0xFA)),
        );
    }
    for [a, b] in bars {
        painter.line_segment(
            [at(a), at(b)],
            egui::Stroke::new((size * 0.062).max(1.5), egui::Color32::from_rgb(0x2F, 0xD4, 0xB4)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_segments() {
        for icon in [
            Icon::ChevronLeft,
            Icon::ChevronRight,
            Icon::ChevronUp,
            Icon::ChevronDown,
            Icon::X,
            Icon::Plus,
        ] {
            let segs = segments(icon, 5.0);
            assert!(!segs.is_empty(), "{:?} must paint something", icon);
            // Segments stay within a reasonable box.
            for [[x1, y1], [x2, y2]] in segs {
                for v in [x1, y1, x2, y2] {
                    assert!(v.abs() <= 5.0 + f32::EPSILON);
                }
            }
        }
    }

    #[test]
    fn logo_png_decodes_32px() {
        let img = image::load_from_memory(LOGO_PNG).expect("baked logo must decode");
        assert_eq!((img.width(), img.height()), (32, 32));
        assert!(logo_image().is_some());
    }

    #[test]
    fn menu_drop_down_reserves_chevron_space() {
        // The painted chevron must not sit on top of the label text: two
        // trailing NBSPs have to widen a real menu_button. Fails loudly if
        // egui ever trims them from measurement.
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let (plain_w, padded_w) = egui::CentralPanel::default()
                .show(ctx, |ui| {
                    let a = ui
                        .menu_button("Riwayat", |_| {})
                        .response
                        .rect
                        .width();
                    let b = menu_drop_down(ui, "Riwayat", |_| {}).response.rect.width();
                    (a, b)
                })
                .inner;
            assert!(
                padded_w > plain_w + 4.0,
                "NBSP padding must widen the button (plain={}, padded={})",
                plain_w,
                padded_w
            );
        });
    }

    #[test]
    fn logo_fallback_geometry_in_bounds() {
        let (legs, bars) = logo_shapes(32.0);
        assert_eq!(legs.len(), 2);
        assert_eq!(bars.len(), 3);
        for [a, b] in legs.into_iter().chain(bars) {
            for [x, y] in [a, b] {
                assert!((0.0..=32.0).contains(&x), "x={}", x);
                assert!((0.0..=32.0).contains(&y), "y={}", y);
            }
        }
    }
}
