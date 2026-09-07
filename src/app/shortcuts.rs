// English comments: configurable keyboard shortcuts (klogg parity).
// A data table drives matching, the editor, and persistence, so adding an
// action cannot desync help text from behavior. Legacy default semantics
// (which extra modifiers were tolerated) are encoded per action and only
// apply to defaults; user-recorded bindings match exactly what was pressed.

use std::collections::HashMap;

use super::*;

/// Modifier set: required, or tolerated as extra.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A resolved binding: key + required mods + tolerated extra mods.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ParsedBinding {
    pub key: egui::Key,
    pub req: Mods,
    pub extra: Mods,
}

/// One configurable action. Scope flags mirror the legacy guards exactly:
/// `needs_tab` (needs an open tab), `allow_typing` (fires while typing in a
/// text field), `allow_dialog` (fires while a dialog/panel is open).
pub struct ShortcutDef {
    pub id: &'static str,
    /// Indonesian source for `lang.tr` (also the English fallback text).
    pub label_id: &'static str,
    pub key: egui::Key,
    pub req_ctrl: bool,
    pub req_shift: bool,
    pub req_alt: bool,
    pub extra_ctrl: bool,
    pub extra_shift: bool,
    pub extra_alt: bool,
    pub needs_tab: bool,
    pub allow_typing: bool,
    pub allow_dialog: bool,
}

/// Actions intentionally NOT in the table (fixed behavior, documented in the
/// editor footer): Escape (dialog safety — unbinding it could trap dialogs)
/// and the 1–9 quick color labels (dynamic group bound to the active query).
pub const SHORTCUTS: &[ShortcutDef] = &[
    ShortcutDef { id: "open_file", label_id: "Buka file log", key: egui::Key::O, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: true, extra_alt: true, needs_tab: false, allow_typing: true, allow_dialog: true },
    ShortcutDef { id: "help", label_id: "Bantuan / daftar pintasan", key: egui::Key::F1, req_ctrl: false, req_shift: false, req_alt: false, extra_ctrl: true, extra_shift: true, extra_alt: true, needs_tab: false, allow_typing: true, allow_dialog: true },
    ShortcutDef { id: "zen", label_id: "Mode Zen", key: egui::Key::F11, req_ctrl: false, req_shift: false, req_alt: false, extra_ctrl: true, extra_shift: true, extra_alt: true, needs_tab: false, allow_typing: true, allow_dialog: true },
    ShortcutDef { id: "palette", label_id: "Command Palette", key: egui::Key::P, req_ctrl: true, req_shift: true, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: true, needs_tab: false, allow_typing: true, allow_dialog: true },
    ShortcutDef { id: "focus_search", label_id: "Fokus ke kolom Cari", key: egui::Key::F, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: true, needs_tab: false, allow_typing: true, allow_dialog: true },
    ShortcutDef { id: "next_hit", label_id: "Hasil berikutnya", key: egui::Key::F3, req_ctrl: false, req_shift: false, req_alt: false, extra_ctrl: true, extra_shift: true, extra_alt: true, needs_tab: true, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "prev_hit", label_id: "Hasil sebelumnya", key: egui::Key::F3, req_ctrl: false, req_shift: true, req_alt: false, extra_ctrl: true, extra_shift: false, extra_alt: true, needs_tab: true, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "next_hit_vi", label_id: "Hasil berikut/plin (gaya vi)", key: egui::Key::N, req_ctrl: false, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: true, extra_alt: false, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "goto", label_id: "Ke baris / persen / akhir / waktu", key: egui::Key::G, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: true, extra_alt: true, needs_tab: true, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "export", label_id: "Ekspor hasil pencarian", key: egui::Key::E, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: true, extra_alt: true, needs_tab: true, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "home", label_id: "Awal file", key: egui::Key::Home, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: true, extra_alt: true, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "end", label_id: "Akhir file", key: egui::Key::End, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: true, extra_alt: true, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "follow", label_id: "Ikuti akhir file (LIVE)", key: egui::Key::F, req_ctrl: true, req_shift: true, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: true, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "hist_back", label_id: "History mundur", key: egui::Key::ArrowLeft, req_ctrl: false, req_shift: false, req_alt: true, extra_ctrl: true, extra_shift: true, extra_alt: false, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "hist_fwd", label_id: "History maju", key: egui::Key::ArrowRight, req_ctrl: false, req_shift: false, req_alt: true, extra_ctrl: true, extra_shift: true, extra_alt: false, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "mark_prev", label_id: "Penanda sebelumnya", key: egui::Key::ArrowUp, req_ctrl: false, req_shift: false, req_alt: true, extra_ctrl: true, extra_shift: true, extra_alt: false, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "mark_next", label_id: "Penanda berikutnya", key: egui::Key::ArrowDown, req_ctrl: false, req_shift: false, req_alt: true, extra_ctrl: true, extra_shift: true, extra_alt: false, needs_tab: true, allow_typing: false, allow_dialog: false },
    ShortcutDef { id: "bookmark", label_id: "Tandai baris aktif", key: egui::Key::B, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: true, needs_tab: true, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "marks_panel", label_id: "Panel penanda", key: egui::Key::B, req_ctrl: true, req_shift: true, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: true, needs_tab: true, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "rename", label_id: "Ubah label penanda", key: egui::Key::F2, req_ctrl: false, req_shift: false, req_alt: false, extra_ctrl: true, extra_shift: true, extra_alt: true, needs_tab: true, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "tab_next", label_id: "Pindah tab (berlaku juga saat mengetik)", key: egui::Key::Tab, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: true, extra_alt: false, needs_tab: false, allow_typing: true, allow_dialog: true },
    ShortcutDef { id: "tab_prev", label_id: "Tab sebelumnya", key: egui::Key::Tab, req_ctrl: true, req_shift: true, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: false, needs_tab: false, allow_typing: true, allow_dialog: true },
    ShortcutDef { id: "zoom_in", label_id: "Zoom UI", key: egui::Key::Equals, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: false, needs_tab: false, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "zoom_out", label_id: "Perkecil", key: egui::Key::Minus, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: false, needs_tab: false, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "zoom_reset", label_id: "Reset zoom", key: egui::Key::Num0, req_ctrl: true, req_shift: false, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: false, needs_tab: false, allow_typing: true, allow_dialog: false },
    ShortcutDef { id: "split", label_id: "Panel belah (dual-pane hasil)", key: egui::Key::S, req_ctrl: true, req_shift: true, req_alt: false, extra_ctrl: false, extra_shift: false, extra_alt: false, needs_tab: false, allow_typing: true, allow_dialog: true },
];

/// Canonical display string for a default binding, e.g. "Ctrl+Shift+P".
pub fn default_string(def: &ShortcutDef) -> String {
    binding_string(
        def.key,
        Mods { ctrl: def.req_ctrl, shift: def.req_shift, alt: def.req_alt },
    )
}

/// Canonical "Ctrl+Shift+P" rendering for a key + required mods.
pub fn binding_string(key: egui::Key, req: Mods) -> String {
    let mut s = String::new();
    if req.ctrl {
        s.push_str("Ctrl+");
    }
    if req.shift {
        s.push_str("Shift+");
    }
    if req.alt {
        s.push_str("Alt+");
    }
    s.push_str(key_name(key));
    s
}

/// Parse "Ctrl+Shift+P" (also "Control+", case-insensitive, spaces ignored).
/// Returns key + required mods; recorded bindings always match exactly.
pub fn parse_binding(s: &str) -> Option<(egui::Key, Mods)> {
    let mut req = Mods::default();
    let mut key: Option<egui::Key> = None;
    for tok in s.split('+') {
        let t = tok.trim().to_ascii_lowercase();
        if t.is_empty() {
            continue;
        }
        match t.as_str() {
            "ctrl" | "control" => req.ctrl = true,
            "shift" => req.shift = true,
            "alt" => req.alt = true,
            _ => {
                if key.is_some() {
                    return None; // two keys: not a chord we support
                }
                // Unknown tokens reject the whole chord (silently dropping
                // a modifier would bind something the user never pressed).
                key = Some(key_from_name(&t)?);
            }
        }
    }
    key.map(|k| (k, req))
}

/// Modifier state snapshot (excludes the Command key on purpose: portable
/// builds treat Ctrl as the command modifier on every OS).
pub fn live_mods(ctx: &egui::Context) -> Mods {
    ctx.input(|i| Mods {
        ctrl: i.modifiers.ctrl,
        shift: i.modifiers.shift,
        alt: i.modifiers.alt,
    })
}

/// True when the binding fires this frame: key pressed, required mods held,
/// non-required mods held only if tolerated.
pub fn binding_fired(ctx: &egui::Context, b: &ParsedBinding) -> bool {
    if !ctx.input(|i| i.key_pressed(b.key)) {
        return false;
    }
    let m = live_mods(ctx);
    if b.req.ctrl && !m.ctrl {
        return false;
    }
    if b.req.shift && !m.shift {
        return false;
    }
    if b.req.alt && !m.alt {
        return false;
    }
    if m.ctrl && !b.req.ctrl && !b.extra.ctrl {
        return false;
    }
    if m.shift && !b.req.shift && !b.extra.shift {
        return false;
    }
    if m.alt && !b.req.alt && !b.extra.alt {
        return false;
    }
    true
}

/// egui key <-> short display name. Covers letters, digits, F1–F12 and the
/// navigation/editing keys the table uses; anything else is not recordable.
pub fn key_name(k: egui::Key) -> &'static str {
    match k {
        egui::Key::A => "A",
        egui::Key::B => "B",
        egui::Key::C => "C",
        egui::Key::D => "D",
        egui::Key::E => "E",
        egui::Key::F => "F",
        egui::Key::G => "G",
        egui::Key::H => "H",
        egui::Key::I => "I",
        egui::Key::J => "J",
        egui::Key::K => "K",
        egui::Key::L => "L",
        egui::Key::M => "M",
        egui::Key::N => "N",
        egui::Key::O => "O",
        egui::Key::P => "P",
        egui::Key::Q => "Q",
        egui::Key::R => "R",
        egui::Key::S => "S",
        egui::Key::T => "T",
        egui::Key::U => "U",
        egui::Key::V => "V",
        egui::Key::W => "W",
        egui::Key::X => "X",
        egui::Key::Y => "Y",
        egui::Key::Z => "Z",
        egui::Key::Num0 => "0",
        egui::Key::Num1 => "1",
        egui::Key::Num2 => "2",
        egui::Key::Num3 => "3",
        egui::Key::Num4 => "4",
        egui::Key::Num5 => "5",
        egui::Key::Num6 => "6",
        egui::Key::Num7 => "7",
        egui::Key::Num8 => "8",
        egui::Key::Num9 => "9",
        egui::Key::F1 => "F1",
        egui::Key::F2 => "F2",
        egui::Key::F3 => "F3",
        egui::Key::F4 => "F4",
        egui::Key::F5 => "F5",
        egui::Key::F6 => "F6",
        egui::Key::F7 => "F7",
        egui::Key::F8 => "F8",
        egui::Key::F9 => "F9",
        egui::Key::F10 => "F10",
        egui::Key::F11 => "F11",
        egui::Key::F12 => "F12",
        egui::Key::Tab => "Tab",
        egui::Key::Space => "Space",
        egui::Key::Enter => "Enter",
        egui::Key::Escape => "Esc",
        egui::Key::ArrowUp => "Up",
        egui::Key::ArrowDown => "Down",
        egui::Key::ArrowLeft => "Left",
        egui::Key::ArrowRight => "Right",
        egui::Key::Home => "Home",
        egui::Key::End => "End",
        egui::Key::PageUp => "PgUp",
        egui::Key::PageDown => "PgDn",
        egui::Key::Equals => "=",
        egui::Key::Minus => "-",
        _ => "?",
    }
}

/// Inverse of `key_name` (lowercase input). None for unsupported keys.
pub fn key_from_name(s: &str) -> Option<egui::Key> {
    Some(match s {
        "a" => egui::Key::A,
        "b" => egui::Key::B,
        "c" => egui::Key::C,
        "d" => egui::Key::D,
        "e" => egui::Key::E,
        "f" => egui::Key::F,
        "g" => egui::Key::G,
        "h" => egui::Key::H,
        "i" => egui::Key::I,
        "j" => egui::Key::J,
        "k" => egui::Key::K,
        "l" => egui::Key::L,
        "m" => egui::Key::M,
        "n" => egui::Key::N,
        "o" => egui::Key::O,
        "p" => egui::Key::P,
        "q" => egui::Key::Q,
        "r" => egui::Key::R,
        "s" => egui::Key::S,
        "t" => egui::Key::T,
        "u" => egui::Key::U,
        "v" => egui::Key::V,
        "w" => egui::Key::W,
        "x" => egui::Key::X,
        "y" => egui::Key::Y,
        "z" => egui::Key::Z,
        "0" => egui::Key::Num0,
        "1" => egui::Key::Num1,
        "2" => egui::Key::Num2,
        "3" => egui::Key::Num3,
        "4" => egui::Key::Num4,
        "5" => egui::Key::Num5,
        "6" => egui::Key::Num6,
        "7" => egui::Key::Num7,
        "8" => egui::Key::Num8,
        "9" => egui::Key::Num9,
        "f1" => egui::Key::F1,
        "f2" => egui::Key::F2,
        "f3" => egui::Key::F3,
        "f4" => egui::Key::F4,
        "f5" => egui::Key::F5,
        "f6" => egui::Key::F6,
        "f7" => egui::Key::F7,
        "f8" => egui::Key::F8,
        "f9" => egui::Key::F9,
        "f10" => egui::Key::F10,
        "f11" => egui::Key::F11,
        "f12" => egui::Key::F12,
        "tab" => egui::Key::Tab,
        "space" => egui::Key::Space,
        "enter" => egui::Key::Enter,
        "esc" | "escape" => egui::Key::Escape,
        "up" | "arrowup" => egui::Key::ArrowUp,
        "down" | "arrowdown" => egui::Key::ArrowDown,
        "left" | "arrowleft" => egui::Key::ArrowLeft,
        "right" | "arrowright" => egui::Key::ArrowRight,
        "home" => egui::Key::Home,
        "end" => egui::Key::End,
        "pgup" | "pageup" => egui::Key::PageUp,
        "pgdn" | "pagedown" => egui::Key::PageDown,
        "=" | "equals" | "plus" => egui::Key::Equals,
        "-" | "minus" => egui::Key::Minus,
        _ => return None,
    })
}

/// Build the runtime table: defaults, with stored overrides applied.
/// Unknown ids and unparsable overrides are ignored (never break input).
pub fn compile_all(overrides: &HashMap<String, String>) -> HashMap<String, ParsedBinding> {
    let mut out = HashMap::new();
    for def in SHORTCUTS {
        let parsed = match overrides.get(def.id) {
            Some(s) => match parse_binding(s) {
                // Recorded bindings match exactly what was pressed.
                Some((key, req)) => ParsedBinding {
                    key,
                    req,
                    extra: Mods::default(),
                },
                None => continue,
            },
            None => ParsedBinding {
                key: def.key,
                req: Mods {
                    ctrl: def.req_ctrl,
                    shift: def.req_shift,
                    alt: def.req_alt,
                },
                extra: Mods {
                    ctrl: def.extra_ctrl,
                    shift: def.extra_shift,
                    alt: def.extra_alt,
                },
            },
        };
        out.insert(def.id.to_string(), parsed);
    }
    out
}

impl AsisLogApp {
    /// Rebuild the compiled table after load/edit/reset.
    pub(crate) fn rebuild_shortcuts(&mut self) {
        self.scut_compiled = compile_all(&self.scut_overrides);
    }

    /// Effective display string for an action (override or default).
    pub(crate) fn scut_label(&self, id: &str) -> String {
        if let Some(s) = self.scut_overrides.get(id) {
            return s.clone();
        }
        SHORTCUTS
            .iter()
            .find(|d| d.id == id)
            .map(default_string)
            .unwrap_or_default()
    }

    /// True when the action's binding fires this frame AND its scope allows
    /// the current context (tab presence, typing, dialogs). Centralizes the
    /// legacy guard matrix so behavior cannot drift per call site.
    pub(crate) fn scut_pressed(
        &self,
        ctx: &egui::Context,
        id: &str,
        has_tab: bool,
        typing: bool,
        dialog_open: bool,
    ) -> bool {
        let Some(def) = SHORTCUTS.iter().find(|d| d.id == id) else {
            return false;
        };
        if def.needs_tab && !has_tab {
            return false;
        }
        if !def.allow_typing && typing {
            return false;
        }
        if !def.allow_dialog && dialog_open {
            return false;
        }
        let Some(b) = self.scut_compiled.get(id) else {
            return false; // unparsable override: unbound, input stays safe
        };
        binding_fired(ctx, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_format_roundtrip() {
        for def in SHORTCUTS {
            let s = default_string(def);
            let (key, req) = parse_binding(&s).unwrap_or_else(|| panic!("unparsable default {}", s));
            assert_eq!(key, def.key, "default {}", def.id);
            assert_eq!(
                (req.ctrl, req.shift, req.alt),
                (def.req_ctrl, def.req_shift, def.req_alt),
                "default {}",
                def.id
            );
            // Canonical form is stable.
            assert_eq!(binding_string(key, req), s, "canonical {}", def.id);
        }
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_binding("").is_none());
        assert!(parse_binding("Ctrl+").is_none());
        assert!(parse_binding("Ctrl+P+Q").is_none());
        assert!(parse_binding("Ctrl+F13").is_none());
        assert!(parse_binding("Hyper+X").is_none());
    }

    #[test]
    fn overrides_replace_defaults() {
        let mut over = HashMap::new();
        over.insert("goto".to_string(), "Alt+G".to_string());
        over.insert("bogus-id".to_string(), "Ctrl+Q".to_string());
        over.insert("export".to_string(), "nonsense!!".to_string());
        let t = compile_all(&over);
        let g = t.get("goto").unwrap();
        assert_eq!(g.key, egui::Key::G);
        assert!(g.req.alt && !g.req.ctrl);
        // Invalid override falls back to... nothing (action unbound, input safe).
        assert!(!t.contains_key("export"));
        assert!(!t.contains_key("bogus-id"));
        // Untouched actions keep defaults.
        assert_eq!(t.get("help").unwrap().key, egui::Key::F1);
    }

    #[test]
    fn key_names_cover_table() {
        for def in SHORTCUTS {
            assert_ne!(key_name(def.key), "?", "unnamed key for {}", def.id);
            assert_eq!(key_from_name(&key_name(def.key).to_lowercase()), Some(def.key));
        }
    }
}
