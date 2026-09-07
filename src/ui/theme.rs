// English comments: selectable color themes (Indonesian names for UI).

/// Pilihan tema warna tampilan.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tema {
    /// Mengikuti tema OS (terang/gelap sistem).
    #[default]
    Sistem,
    /// Gelap.
    Gelap,
    /// Terang.
    Terang,
    /// Kontras tinggi (latar hitam, teks putih, aksen kuning).
    KontrasTinggi,
    /// Senja biru (gelap kebiruan).
    SenjaBiru,
    /// Terang kontras (teks hitam pekat, aksen tegas).
    TerangKontras,
    /// Solarized gelap.
    SolarGelap,
    /// Solarized terang.
    SolarTerang,
    /// Monokai (gelap kehangatan editor).
    Monokai,
}

impl Tema {
    /// Semua pilihan, untuk ComboBox.
    pub fn semua() -> &'static [Tema] {
        &[
            Tema::Sistem,
            Tema::Gelap,
            Tema::Terang,
            Tema::TerangKontras,
            Tema::KontrasTinggi,
            Tema::SenjaBiru,
            Tema::SolarGelap,
            Tema::SolarTerang,
            Tema::Monokai,
        ]
    }

    /// Nama Indonesia untuk UI.
    pub fn nama(self) -> &'static str {
        self.nama_in(crate::i18n::Lang::Id)
    }

    /// Language-aware display name (keys stable for config).
    pub fn nama_in(self, lang: crate::i18n::Lang) -> &'static str {
        // Reuse the central dictionary so ID/EN stay in sync.
        lang.tr(match self {
            Tema::Sistem => "Sistem (otomatis)",
            Tema::Gelap => "Gelap",
            Tema::Terang => "Terang",
            Tema::TerangKontras => "Terang kontras",
            Tema::KontrasTinggi => "Kontras tinggi",
            Tema::SenjaBiru => "Senja biru",
            Tema::SolarGelap => "Solarized gelap",
            Tema::SolarTerang => "Solarized terang",
            Tema::Monokai => "Monokai",
        })
    }

    /// Kunci config untuk persistensi.
    pub fn key(self) -> &'static str {
        match self {
            Tema::Sistem => "system",
            Tema::Gelap => "dark",
            Tema::Terang => "light",
            Tema::TerangKontras => "light_contrast",
            Tema::KontrasTinggi => "contrast",
            Tema::SenjaBiru => "dusk",
            Tema::SolarGelap => "solar_dark",
            Tema::SolarTerang => "solar_light",
            Tema::Monokai => "monokai",
        }
    }

    pub fn from_key(s: &str) -> Tema {
        match s {
            "dark" => Tema::Gelap,
            "light" => Tema::Terang,
            "light_contrast" => Tema::TerangKontras,
            "contrast" => Tema::KontrasTinggi,
            "dusk" => Tema::SenjaBiru,
            "solar_dark" => Tema::SolarGelap,
            "solar_light" => Tema::SolarTerang,
            "monokai" => Tema::Monokai,
            _ => Tema::Sistem,
        }
    }

    /// True untuk varian gelap (mempengaruhi warna teks log).
    /// `Sistem` diasumsikan gelap sampai sempat dibaca dari OS.
    pub fn is_dark(self) -> bool {
        match self {
            Tema::Gelap
            | Tema::KontrasTinggi
            | Tema::SenjaBiru
            | Tema::SolarGelap
            | Tema::Monokai => true,
            Tema::Terang | Tema::TerangKontras | Tema::SolarTerang => false,
            Tema::Sistem => true,
        }
    }

    /// Selesaikan `Sistem` dari tema OS: (visuals, dark).
    /// OS terang → Terang; gelap/tak diketahui → Gelap.
    pub fn resolve(self, ctx: &egui::Context) -> (egui::Visuals, bool) {
        match self {
            Tema::Sistem => match ctx.system_theme() {
                Some(egui::Theme::Light) => (Tema::Terang.visuals(), false),
                _ => (Tema::Gelap.visuals(), true),
            },
            _ => (self.visuals(), self.is_dark()),
        }
    }

    /// egui visuals untuk tema ini.
    /// `Sistem` tanpa konteks OS memakai gelap; gunakan `resolve()` bila ada ctx.
    pub fn visuals(self) -> egui::Visuals {
        match self {
            Tema::Sistem | Tema::Gelap => egui::Visuals::dark(),
            Tema::Terang => egui::Visuals::light(),
            Tema::KontrasTinggi => {
                let mut v = egui::Visuals::dark();
                v.override_text_color = Some(egui::Color32::WHITE);
                v.panel_fill = egui::Color32::BLACK;
                v.extreme_bg_color = egui::Color32::BLACK;
                v.code_bg_color = egui::Color32::BLACK;
                v.selection.bg_fill = egui::Color32::from_rgb(255, 220, 0);
                v.selection.stroke.color = egui::Color32::BLACK;
                v.warn_fg_color = egui::Color32::YELLOW;
                v.error_fg_color = egui::Color32::from_rgb(255, 120, 120);
                v
            }
            Tema::SenjaBiru => {
                let mut v = egui::Visuals::dark();
                v.panel_fill = egui::Color32::from_rgb(18, 28, 48);
                v.extreme_bg_color = egui::Color32::from_rgb(10, 16, 30);
                v.code_bg_color = egui::Color32::from_rgb(10, 16, 30);
                // Kontras dinaikkan: teks putih, seleksi/hover biru tegas.
                v.override_text_color = Some(egui::Color32::from_rgb(235, 242, 252));
                v.selection.bg_fill = egui::Color32::from_rgb(70, 130, 205);
                v.selection.stroke.color = egui::Color32::WHITE;
                v.widgets.hovered.bg_fill = egui::Color32::from_rgb(38, 56, 88);
                v.widgets.active.bg_fill = egui::Color32::from_rgb(60, 105, 175);
                v
            }
            Tema::TerangKontras => {
                let mut v = egui::Visuals::light();
                v.override_text_color = Some(egui::Color32::BLACK);
                v.panel_fill = egui::Color32::WHITE;
                v.extreme_bg_color = egui::Color32::from_rgb(240, 240, 240);
                v.selection.bg_fill = egui::Color32::from_rgb(23, 105, 170);
                v.selection.stroke.color = egui::Color32::WHITE;
                v
            }
            Tema::SolarGelap => {
                let mut v = egui::Visuals::dark();
                v.panel_fill = egui::Color32::from_rgb(0, 43, 54);
                v.extreme_bg_color = egui::Color32::from_rgb(7, 54, 66);
                v.code_bg_color = egui::Color32::from_rgb(7, 54, 66);
                v.override_text_color = Some(egui::Color32::from_rgb(131, 148, 150));
                v.selection.bg_fill = egui::Color32::from_rgb(38, 139, 210);
                v.selection.stroke.color = egui::Color32::WHITE;
                v
            }
            Tema::SolarTerang => {
                let mut v = egui::Visuals::light();
                v.panel_fill = egui::Color32::from_rgb(253, 246, 227);
                v.extreme_bg_color = egui::Color32::from_rgb(238, 232, 213);
                v.override_text_color = Some(egui::Color32::from_rgb(60, 70, 75));
                v.selection.bg_fill = egui::Color32::from_rgb(38, 139, 210);
                v.selection.stroke.color = egui::Color32::WHITE;
                v
            }
            Tema::Monokai => {
                let mut v = egui::Visuals::dark();
                v.panel_fill = egui::Color32::from_rgb(39, 40, 34);
                v.extreme_bg_color = egui::Color32::from_rgb(30, 31, 28);
                v.code_bg_color = egui::Color32::from_rgb(30, 31, 28);
                v.override_text_color = Some(egui::Color32::from_rgb(248, 248, 242));
                v.selection.bg_fill = egui::Color32::from_rgb(117, 113, 94);
                v.selection.stroke.color = egui::Color32::WHITE;
                v.warn_fg_color = egui::Color32::from_rgb(253, 151, 31);
                v.error_fg_color = egui::Color32::from_rgb(249, 38, 114);
                v
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semua_tema_punya_nama() {
        for t in Tema::semua() {
            assert!(!t.nama().is_empty());
        }
        assert_eq!(Tema::default(), Tema::Sistem);
    }

    #[test]
    fn terang_itu_light() {
        assert!(!Tema::Terang.is_dark());
        assert!(!Tema::TerangKontras.is_dark());
        assert!(!Tema::SolarTerang.is_dark());
        assert!(Tema::Gelap.is_dark());
        assert!(Tema::KontrasTinggi.is_dark());
        assert!(Tema::SolarGelap.is_dark());
        assert!(Tema::Monokai.is_dark());
    }

    #[test]
    fn key_roundtrip() {
        for t in Tema::semua() {
            assert_eq!(Tema::from_key(t.key()), *t);
        }
    }
}
