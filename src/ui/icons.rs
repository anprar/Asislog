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
}
