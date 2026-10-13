//! Interface themes, adapted from LightCraft's theme (see NOTICE) so the app
//! looks like the ArtCraft applications. The photo-specific tokens and the CJK
//! font fallbacks were dropped.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};

pub const FONT_SEMIBOLD: &str = "semibold";

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    /// Side panel, top bar and status bar.
    pub chrome: Color32,
    /// Central area behind the step pages.
    pub canvas: Color32,
    pub divider: Color32,
    pub inset: Color32,
    pub field: Color32,
    pub button: Color32,
    pub button_border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub text: Color32,
    pub text_label: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub good: Color32,
    pub bad: Color32,
    pub caution: Color32,
    pub panel_w: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeKind {
    #[default]
    Charcoal,
    Midnight,
    Silver,
    Paper,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 4] = [ThemeKind::Charcoal, ThemeKind::Midnight, ThemeKind::Silver, ThemeKind::Paper];

    pub fn is_dark(self) -> bool {
        matches!(self, ThemeKind::Charcoal | ThemeKind::Midnight)
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Charcoal => "Charcoal",
            ThemeKind::Midnight => "Midnight",
            ThemeKind::Silver => "Silver",
            ThemeKind::Paper => "Paper",
        }
    }
}

impl Default for Tokens {
    fn default() -> Self {
        let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        Tokens {
            chrome: rgb(0x2d2d2d),
            canvas: rgb(0x1c1c1c),
            divider: rgb(0x1c1c1c),
            inset: rgb(0x232323),
            field: rgb(0x222222),
            button: rgb(0x242424),
            button_border: rgb(0x4c4c4c),
            hover: rgb(0x3a3a3a),
            pressed: rgb(0x3f3f3f),
            text: rgb(0xe2e2e2),
            text_label: rgb(0xbcbcbc),
            text_dim: rgb(0x8e8e8e),
            accent: rgb(0x0165dd),
            good: rgb(0x4caf6a),
            bad: rgb(0xe04a4a),
            caution: rgb(0xe3a83c),
            panel_w: 230.0,
        }
    }
}

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Tokens {
        let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        let base = Tokens::default();
        match kind {
            ThemeKind::Charcoal => base,
            ThemeKind::Midnight => Tokens {
                chrome: rgb(0x1e1e1e),
                canvas: rgb(0x121212),
                divider: rgb(0x0e0e0e),
                inset: rgb(0x181818),
                field: rgb(0x161616),
                button: rgb(0x191919),
                button_border: rgb(0x404040),
                hover: rgb(0x2e2e2e),
                pressed: rgb(0x343434),
                ..base
            },
            ThemeKind::Silver => Tokens {
                chrome: rgb(0xe6e6e6),
                canvas: rgb(0xcfcfcf),
                divider: rgb(0xc4c4c4),
                inset: rgb(0xdcdcdc),
                field: rgb(0xf6f6f6),
                button: rgb(0xf2f2f2),
                button_border: rgb(0xb0b0b0),
                hover: rgb(0xd2d2d2),
                pressed: rgb(0xc4c4c4),
                text: rgb(0x1c1c1c),
                text_label: rgb(0x383838),
                text_dim: rgb(0x6a6a6a),
                accent: rgb(0x0a62d0),
                good: rgb(0x2e8a4a),
                bad: rgb(0xc83434),
                caution: rgb(0xb0761a),
                ..base
            },
            ThemeKind::Paper => Tokens {
                chrome: rgb(0xfafafa),
                canvas: rgb(0xececec),
                divider: rgb(0xe0e0e0),
                inset: rgb(0xf0f0f0),
                field: rgb(0xffffff),
                button: rgb(0xffffff),
                button_border: rgb(0xc6c6c6),
                hover: rgb(0xebebeb),
                pressed: rgb(0xdedede),
                text: rgb(0x161616),
                text_label: rgb(0x343434),
                text_dim: rgb(0x707070),
                accent: rgb(0x0a62d0),
                good: rgb(0x2e8a4a),
                bad: rgb(0xc83434),
                caution: rgb(0xb0761a),
                ..base
            },
        }
    }

    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::NULL)).unwrap_or_default()
    }

    pub fn semibold(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(FONT_SEMIBOLD.into()))
    }
}

/// Inter first, egui's own faces as fallback for other scripts.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "Inter-SemiBold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.ttf"))),
    );
    let defaults: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = vec!["Inter".to_string()];
    prop.extend(defaults.iter().cloned());
    fonts.families.insert(FontFamily::Proportional, prop);
    let mut semi = vec!["Inter-SemiBold".to_string()];
    semi.extend(defaults);
    fonts.families.insert(FontFamily::Name(FONT_SEMIBOLD.into()), semi);
    ctx.set_fonts(fonts);
}

/// Style `ctx` with `kind`. The visuals go to both of egui's styles, so egui
/// following the desktop's dark/light setting never swaps in its stock look.
pub fn apply_kind(ctx: &egui::Context, kind: ThemeKind) {
    let t = Tokens::for_kind(kind);
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, t));
    let dark = kind.is_dark();
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.chrome;
    v.window_fill = t.chrome;
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.inset;
    v.window_stroke = Stroke::new(1.0, t.button_border);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = t.accent.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.override_text_color = Some(t.text_label);
    let (popup, window) = if dark { (140, 160) } else { (50, 60) };
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(popup) };
    v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(window) };
    let w = &mut v.widgets;
    craft_ui::theme::WidgetPalette { noninteractive: t.chrome, inactive: t.button, hovered: t.hover, active: t.pressed, open: t.hover }
        .apply_to(w, CornerRadius::same(4), Stroke::new(1.0, t.text_label));
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    w.inactive.bg_stroke = Stroke::new(1.0, t.button_border);
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::new(16.0, FontFamily::Name(FONT_SEMIBOLD.into())));
        s.animation_time = 0.08;
    });
}
