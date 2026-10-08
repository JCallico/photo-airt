//! Visual identity: a dark, warm "gallery at night" theme with gold for the
//! algorithms, terracotta for Claude, teal for Codex and violet for duets.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText, Stroke, Vec2};

pub const BG: Color32 = Color32::from_rgb(12, 11, 14);
pub const PANEL: Color32 = Color32::from_rgb(20, 18, 23);
pub const CARD: Color32 = Color32::from_rgb(28, 26, 32);
pub const CARD_HI: Color32 = Color32::from_rgb(37, 34, 42);
pub const LINE: Color32 = Color32::from_rgb(46, 42, 52);
pub const TEXT: Color32 = Color32::from_rgb(236, 230, 218);
pub const MUTED: Color32 = Color32::from_rgb(150, 142, 133);
pub const FAINT: Color32 = Color32::from_rgb(95, 90, 96);
pub const GOLD: Color32 = Color32::from_rgb(227, 177, 92);
pub const CLAUDE: Color32 = Color32::from_rgb(217, 119, 87);
pub const CODEX: Color32 = Color32::from_rgb(63, 190, 172);
pub const DUET: Color32 = Color32::from_rgb(180, 140, 255);
pub const DANGER: Color32 = Color32::from_rgb(232, 98, 98);

pub fn serif() -> FontFamily {
    FontFamily::Name("serif".into())
}

pub fn serif_italic() -> FontFamily {
    FontFamily::Name("serif-italic".into())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), FontData::from_static(include_bytes!("../assets/fonts/Inter.ttf")).into());
    fonts.font_data.insert("playfair".into(), FontData::from_static(include_bytes!("../assets/fonts/PlayfairDisplay.ttf")).into());
    fonts
        .font_data
        .insert("playfair-italic".into(), FontData::from_static(include_bytes!("../assets/fonts/PlayfairDisplay-Italic.ttf")).into());
    let fallbacks: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "inter".into());
    let mut serif_list = vec!["playfair".to_string()];
    serif_list.extend(fallbacks.iter().cloned());
    let mut serif_i = vec!["playfair-italic".to_string()];
    serif_i.extend(fallbacks);
    fonts.families.insert(serif(), serif_list);
    fonts.families.insert(serif_italic(), serif_i);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        use egui::TextStyle::*;
        style.text_styles = [
            (Heading, FontId::new(22.0, serif())),
            (Body, FontId::new(13.5, FontFamily::Proportional)),
            (Button, FontId::new(13.0, FontFamily::Proportional)),
            (Small, FontId::new(11.0, FontFamily::Proportional)),
            (Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(8.0, 7.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.slider_width = 170.0;
        style.spacing.interact_size.y = 22.0;
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.extreme_bg_color = Color32::from_rgb(16, 15, 19);
        v.faint_bg_color = CARD;
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = GOLD;
        v.selection.bg_fill = GOLD.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, GOLD);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1.0, LINE);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        let r = CornerRadius::same(7);
        for (w, fill, stroke) in [
            (&mut v.widgets.noninteractive, CARD, LINE),
            (&mut v.widgets.inactive, CARD_HI, LINE),
            (&mut v.widgets.hovered, Color32::from_rgb(48, 44, 54), Color32::from_rgb(80, 72, 84)),
            (&mut v.widgets.active, Color32::from_rgb(58, 52, 62), GOLD),
            (&mut v.widgets.open, CARD_HI, LINE),
        ] {
            w.corner_radius = r;
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0, stroke);
        }
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT);
        v.widgets.active.fg_stroke = Stroke::new(1.5, GOLD);
    });
}

pub fn label_caps(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).size(10.5).color(MUTED).extra_letter_spacing(1.6).strong()
}

pub fn title(text: &str, size: f32) -> RichText {
    RichText::new(text).family(serif()).size(size).color(TEXT)
}

pub fn title_italic(text: &str, size: f32) -> RichText {
    RichText::new(text).family(serif_italic()).size(size).color(TEXT)
}

pub fn card_frame(accent: Option<Color32>) -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
        .stroke(Stroke::new(1.0, accent.map(|c| c.gamma_multiply(0.45)).unwrap_or(LINE)))
}

/// Gradient pill button with an accent colour.
pub fn accent_button(ui: &mut egui::Ui, text: &str, accent: Color32, enabled: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.5), Color32::WHITE);
    let size = Vec2::new((galley.size().x + 30.0).max(ui.available_width().min(160.0)), 32.0);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered() && enabled);
    let painter = ui.painter();
    let base = if enabled { lerp_color(accent.gamma_multiply(0.85), accent, hover) } else { CARD_HI };
    painter.rect_filled(rect, CornerRadius::same(16), base);
    if enabled {
        // Soft top sheen.
        let top = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.center().y));
        painter.rect_filled(top, CornerRadius { nw: 16, ne: 16, sw: 0, se: 0 }, Color32::from_white_alpha(22));
    }
    if enabled && hover > 0.0 {
        painter.rect_stroke(
            rect.expand(2.0 * hover),
            CornerRadius::same(18),
            Stroke::new(1.0, accent.gamma_multiply(0.6 * hover)),
            egui::StrokeKind::Outside,
        );
    }
    let color = if enabled { Color32::WHITE } else { FAINT };
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, FontId::proportional(13.5), color);
    resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed })
}

pub fn chip(ui: &mut egui::Ui, text: &str, selected: bool, accent: Color32) -> egui::Response {
    chip_enabled(ui, text, selected, accent, true)
}

/// A chip that can be shown greyed-out (still hoverable so it can explain why).
pub fn chip_enabled(ui: &mut egui::Ui, text: &str, selected: bool, accent: Color32, enabled: bool) -> egui::Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), TEXT);
    let size = Vec2::new(galley.size().x + 18.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered() && enabled);
    let fill = if selected { accent.gamma_multiply(0.28) } else { lerp_color(CARD_HI, Color32::from_rgb(52, 48, 58), hover) };
    let stroke = if selected { accent } else { LINE };
    let color = if !enabled {
        FAINT
    } else if selected {
        Color32::WHITE
    } else {
        TEXT
    };
    ui.painter().rect(rect, CornerRadius::same(12), if enabled { fill } else { CARD }, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, font, color);
    resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed })
}

pub fn pill(ui: &mut egui::Ui, dot: Color32, text: &str) -> egui::Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), TEXT);
    let size = Vec2::new(galley.size().x + 26.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(11), CARD, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    ui.painter().circle_filled(egui::pos2(rect.left() + 11.0, rect.center().y), 3.5, dot);
    ui.painter().text(egui::pos2(rect.left() + 19.0, rect.center().y), egui::Align2::LEFT_CENTER, text, font, TEXT);
    resp
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn engine_color(e: crate::ai::Engine) -> Color32 {
    match e {
        crate::ai::Engine::Claude => CLAUDE,
        crate::ai::Engine::Codex => CODEX,
        crate::ai::Engine::Duet => DUET,
    }
}
