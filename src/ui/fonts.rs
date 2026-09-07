// English comments: system UI + monospace font loading (native feel).
// egui embeds its own fonts; without this the app looks identical (and
// foreign) on every OS. We prepend OS fonts when present and silently keep
// the embedded fallback otherwise — never an error, never a tofu box.

/// Load order matters: first hit wins, embedded egui fonts stay as fallback.
fn system_ui_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Some(windir) = std::env::var_os("WINDIR") {
            out.push(std::path::PathBuf::from(windir).join("Fonts").join("segoeui.ttf"));
        }
        out.push(std::path::PathBuf::from("C:\\Windows\\Fonts\\segoeui.ttf"));
    }
    #[cfg(target_os = "linux")]
    {
        for p in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/opentype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        ] {
            out.push(std::path::PathBuf::from(p));
        }
    }
    #[cfg(target_os = "macos")]
    {
        for p in [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
        ] {
            out.push(std::path::PathBuf::from(p));
        }
    }
    out
}

/// Monospace candidates for a named choice. "Bawaan" (default) = none
/// (keep embedded Hack). Unknown names also resolve to none.
fn mono_candidates(choice: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let files: &[&str] = match choice {
        "JetBrains Mono" => &[
            "JetBrainsMono-Regular.ttf",
            "JetBrains Mono Regular.ttf",
        ],
        "Consolas" => &["consola.ttf", "Consolas.ttf"],
        _ => return out,
    };
    #[cfg(target_os = "windows")]
    {
        let windir =
            std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| {
                std::path::PathBuf::from("C:\\Windows")
            });
        for f in files {
            out.push(windir.join("Fonts").join(f));
        }
        if choice == "JetBrains Mono" {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                out.push(
                    std::path::PathBuf::from(local)
                        .join("Microsoft\\Windows\\Fonts")
                        .join("JetBrainsMono-Regular.ttf"),
                );
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        for dir in [
            "/usr/share/fonts/truetype/jetbrains-mono",
            "/usr/share/fonts/opentype/jetbrains-mono",
            "/usr/share/fonts/truetype/dejavu",
        ] {
            for f in files {
                out.push(std::path::PathBuf::from(dir).join(f));
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            for f in files {
                out.push(
                    std::path::PathBuf::from(&home)
                        .join(".local/share/fonts")
                        .join(f),
                );
            }
        }
        // Consolas metric-compatible fallback present on many distros.
        if choice == "Consolas" {
            out.push(std::path::PathBuf::from(
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            ));
        }
    }
    #[cfg(target_os = "macos")]
    {
        for dir in [
            "/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
        ] {
            for f in files {
                out.push(std::path::PathBuf::from(dir).join(f));
            }
        }
        if choice == "Consolas" {
            out.push(std::path::PathBuf::from("/System/Library/Fonts/Menlo.ttc"));
        }
    }
    out
}

fn load_first(paths: &[std::path::PathBuf]) -> Option<Vec<u8>> {
    for p in paths {
        if let Ok(b) = std::fs::read(p) {
            if !b.is_empty() {
                return Some(b);
            }
        }
    }
    None
}

/// Install fonts into egui: optional system UI font for proportional text,
/// optional named monospace for logs. Missing files = silent fallback to
/// embedded fonts (never errors, never panics — headless-testable).
pub fn install_fonts(ctx: &egui::Context, ui_system: bool, mono_choice: &str) {
    let mut fonts = egui::FontDefinitions::default();
    if ui_system {
        if let Some(bytes) = load_first(&system_ui_candidates()) {
            fonts.font_data.insert(
                "system-ui".to_owned(),
                egui::FontData::from_owned(bytes).into(),
            );
            if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                fam.insert(0, "system-ui".to_owned());
            }
        }
    }
    if let Some(bytes) = load_first(&mono_candidates(mono_choice)) {
        fonts.font_data.insert(
            "mono-choice".to_owned(),
            egui::FontData::from_owned(bytes).into(),
        );
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            fam.insert(0, "mono-choice".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_fall_back_silently() {
        assert!(load_first(&[]).is_none());
        assert!(load_first(&[std::path::PathBuf::from(
            "definitely/not/here-asislog.ttf"
        )])
        .is_none());
        // Unknown mono choice resolves to no candidates (embedded kept).
        assert!(mono_candidates("NoSuchFont XYZ").is_empty());
        assert!(mono_candidates("Bawaan").is_empty());
    }

    #[test]
    fn install_never_panics_headless() {
        let ctx = egui::Context::default();
        install_fonts(&ctx, true, "NoSuchFont XYZ");
        install_fonts(&ctx, false, "Bawaan");
        // Embedded families always survive.
        let defs = egui::FontDefinitions::default();
        assert!(defs.families.contains_key(&egui::FontFamily::Proportional));
        assert!(defs.families.contains_key(&egui::FontFamily::Monospace));
    }
}
