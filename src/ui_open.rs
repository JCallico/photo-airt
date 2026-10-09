//! "Open anywhere" sheet, "All photos" contact sheet and download progress.

use egui::{Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::app::App;
use crate::app_open::{ClipStatus, OpenRequest};
use crate::sources::{self, Input};
use crate::theme::{self, *};

fn cover_uv(size: [usize; 2], w: f32, h: f32) -> Rect {
    let ta = size[0] as f32 / size[1].max(1) as f32;
    let ca = w / h;
    if ta > ca {
        let u0 = (1.0 - ca / ta) / 2.0;
        Rect::from_min_max(pos2(u0, 0.0), pos2(1.0 - u0, 1.0))
    } else {
        let v0 = (1.0 - ta / ca) / 2.0;
        Rect::from_min_max(pos2(0.0, v0), pos2(1.0, 1.0 - v0))
    }
}

fn mb(bytes: u64) -> String {
    if bytes >= 1_048_576 { format!("{:.1} MB", bytes as f64 / 1_048_576.0) } else { format!("{} KB", bytes / 1024) }
}

/// A large clickable source tile.
#[allow(clippy::too_many_arguments)]
fn tile(ui: &mut Ui, w: f32, h: f32, icon: &str, title: &str, detail: &str, accent: Color32, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), if enabled { Sense::click() } else { Sense::hover() });
    let hov = ui.ctx().animate_bool(resp.id, resp.hovered() && enabled);
    let p = ui.painter();
    let fill = theme::lerp_color(CARD, CARD_HI, hov);
    p.rect(rect, CornerRadius::same(12), fill, Stroke::new(1.0, theme::lerp_color(LINE, accent, hov)), egui::StrokeKind::Inside);
    let c = if enabled { TEXT } else { FAINT };
    p.text(rect.left_top() + vec2(16.0, 16.0), Align2::LEFT_TOP, icon, FontId::proportional(24.0), if enabled { accent } else { FAINT });
    p.text(rect.left_top() + vec2(16.0, 54.0), Align2::LEFT_TOP, title, FontId::new(17.0, serif()), c);
    let g = p.layout(detail.to_string(), FontId::proportional(11.5), MUTED, w - 32.0);
    p.galley(rect.left_top() + vec2(16.0, 80.0), g, MUTED);
    resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Default })
}

impl App {
    // ------------------------------------------------------------ open sheet

    pub fn open_sheet_ui(&mut self, ctx: &egui::Context) {
        if !self.open_sheet.visible {
            return;
        }
        // Offer a link found on the clipboard, once.
        if !self.open_sheet.prefilled
            && self.open_sheet.input.is_empty()
            && let ClipStatus::Text(t) = &self.open_sheet.clip
            && sources::classify(t).is_ok()
        {
            self.open_sheet.input = t.lines().next().unwrap_or("").trim().to_string();
            self.open_sheet.prefilled = true;
        }
        if let ClipStatus::Image { preview, .. } = &self.open_sheet.clip
            && self.open_sheet.clip_tex.is_none()
        {
            self.open_sheet.clip_tex = Some(ctx.load_texture("clipboard-preview", preview.clone(), egui::TextureOptions::LINEAR));
        }

        let mut action: Option<OpenRequest> = None;
        let mut browse = false;
        let mut open_recent: Option<usize> = None;
        let mut forget: Option<std::path::PathBuf> = None;
        let mut clear = false;
        let frame = egui::Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(16))
            .stroke(Stroke::new(1.0, LINE))
            .inner_margin(Margin::same(22))
            .shadow(egui::epaint::Shadow { offset: [0, 18], blur: 50, spread: 0, color: Color32::from_black_alpha(170) });
        let modal =
            egui::Modal::new(egui::Id::new("open-sheet")).frame(frame).backdrop_color(Color32::from_black_alpha(150)).show(ctx, |ui| {
                let w = 700.0;
                ui.set_width(w);
                ui.horizontal(|ui| {
                    ui.label(theme::title_italic("Open a photo", 28.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Esc").size(11.0).color(FAINT));
                    });
                });
                ui.label(
                    RichText::new("From your computer, the web or the clipboard. No accounts, nothing to set up.").color(MUTED).size(12.5),
                );
                ui.add_space(12.0);

                // ---- smart bar
                let classified = sources::classify(&self.open_sheet.input);
                ui.horizontal(|ui| {
                    let edit = egui::TextEdit::singleline(&mut self.open_sheet.input)
                        .hint_text("Paste a link (image or web page) or a file path…")
                        .font(FontId::proportional(15.0))
                        .margin(Margin::symmetric(12, 9))
                        .desired_width(w - 110.0);
                    let resp = ui.add(edit);
                    if self.open_sheet.focus {
                        resp.request_focus();
                        self.open_sheet.focus = false;
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let mut child = ui.new_child(
                        egui::UiBuilder::new().max_rect(Rect::from_min_size(ui.cursor().min + vec2(0.0, 2.0), vec2(96.0, 34.0))),
                    );
                    let can = classified.is_ok()
                        || (self.open_sheet.input.trim().is_empty() && matches!(self.open_sheet.clip, ClipStatus::Image { .. }));
                    if theme::accent_button(&mut child, "Open", GOLD, can).clicked() || (enter && can) {
                        action = Some(match &classified {
                            Ok(input) => OpenRequest::Input(input.clone()),
                            Err(_) => OpenRequest::ClipboardImage,
                        });
                    }
                });
                let (icon, line, color) = if self.open_sheet.input.trim().is_empty() {
                    match &self.open_sheet.clip {
                        ClipStatus::Image { width, height, .. } => {
                            ("📋", format!("Press Enter to open the image on your clipboard ({width} × {height})"), GOLD)
                        }
                        _ => ("✦", "Tip: Ctrl+V a link anywhere in the studio, or drop files on the window".to_string(), FAINT),
                    }
                } else {
                    match &classified {
                        Ok(input @ Input::File(p)) => {
                            let (what, detail) = sources::describe(input);
                            ("📁", format!("{what} · {detail}"), if p.is_file() { CODEX } else { DANGER })
                        }
                        Ok(input) => {
                            let (what, detail) = sources::describe(input);
                            ("🌐", format!("{what} · {detail}"), CODEX)
                        }
                        Err(e) => ("⚠", e.clone(), DANGER),
                    }
                };
                ui.add_space(2.0);
                ui.label(RichText::new(format!("{icon}  {line}")).size(12.0).color(color));
                ui.add_space(14.0);

                // ---- source tiles
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let tw = (w - 24.0) / 3.0;
                    if tile(ui, tw, 132.0, "📁", "Files", "Browse your computer. Pick several to open them together.", GOLD, !self.picking)
                        .clicked()
                    {
                        browse = true;
                    }
                    // Clipboard tile shows a live preview of what's there.
                    let (title, detail, enabled) = match &self.open_sheet.clip {
                        ClipStatus::Unknown => ("Clipboard", "Checking…".to_string(), false),
                        ClipStatus::Image { width, height, .. } => ("Paste image", format!("{width} × {height} image ready"), true),
                        ClipStatus::Text(t) if sources::classify(t).is_ok() => {
                            ("Paste link", t.lines().next().unwrap_or("").chars().take(60).collect(), true)
                        }
                        ClipStatus::Text(_) => ("Clipboard", "Holds text that isn't a link or a path".to_string(), false),
                        ClipStatus::Empty => ("Clipboard", "Nothing to paste. Copy an image or a link.".to_string(), false),
                        ClipStatus::Unavailable(e) => ("Clipboard", e.chars().take(70).collect(), false),
                    };
                    let r = tile(ui, tw, 132.0, "📋", title, &detail, CLAUDE, enabled);
                    if let Some(tex) = &self.open_sheet.clip_tex {
                        let pr = Rect::from_min_size(r.rect.right_top() + vec2(-86.0, 12.0), vec2(72.0, 54.0));
                        egui::Image::new(tex).uv(cover_uv(tex.size(), 72.0, 54.0)).corner_radius(CornerRadius::same(6)).paint_at(ui, pr);
                    }
                    if r.clicked() {
                        action = Some(match &self.open_sheet.clip {
                            ClipStatus::Text(t) => match sources::classify(t) {
                                Ok(i) => OpenRequest::Input(i),
                                Err(_) => OpenRequest::ClipboardImage,
                            },
                            _ => OpenRequest::ClipboardImage,
                        });
                    }
                    if tile(
                        ui,
                        tw,
                        132.0,
                        "🌐",
                        "The web",
                        "Image links, or pages with a preview image: Wikipedia, Flickr, Unsplash, news, blogs.",
                        CODEX,
                        true,
                    )
                    .clicked()
                    {
                        self.open_sheet.focus = true;
                    }
                });

                // ---- recents
                if !self.recents.items.is_empty() {
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.label(theme::label_caps("Recent"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Clear").on_hover_text("Forget recent photos and their cached copies").clicked() {
                                clear = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let cols = 5;
                    let cw = (w - 10.0 * (cols as f32 - 1.0)) / cols as f32;
                    let ch = cw * 0.68;
                    let items: Vec<_> = self.recents.items.iter().take(10).cloned().collect();
                    for (row, chunk) in items.chunks(cols).enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            for (k, r) in chunk.iter().enumerate() {
                                let idx = row * cols + k;
                                let (rect, resp) = ui.allocate_exact_size(vec2(cw, ch + 34.0), Sense::click());
                                let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
                                let img_r = Rect::from_min_size(rect.min, vec2(cw, ch));
                                match self.recent_texture(&r.thumb) {
                                    Some(tex) => {
                                        egui::Image::new(&tex)
                                            .uv(cover_uv(tex.size(), cw, ch))
                                            .corner_radius(CornerRadius::same(8))
                                            .paint_at(ui, img_r);
                                    }
                                    None => {
                                        ui.painter().rect_filled(img_r, CornerRadius::same(8), CARD_HI);
                                    }
                                }
                                let p = ui.painter();
                                p.rect_stroke(
                                    img_r,
                                    CornerRadius::same(8),
                                    Stroke::new(1.0 + hov, theme::lerp_color(LINE, GOLD, hov)),
                                    egui::StrokeKind::Inside,
                                );
                                let name: String = r.asset.name.chars().take(22).collect();
                                p.text(
                                    pos2(rect.left() + 2.0, img_r.bottom() + 6.0),
                                    Align2::LEFT_TOP,
                                    name,
                                    FontId::proportional(11.5),
                                    TEXT,
                                );
                                p.text(
                                    pos2(rect.left() + 2.0, img_r.bottom() + 21.0),
                                    Align2::LEFT_TOP,
                                    format!("{} {} · {}", r.asset.origin.icon(), r.asset.origin.label(), sources::ago(r.opened)),
                                    FontId::proportional(10.0),
                                    FAINT,
                                );
                                // Forget button on hover.
                                if hov > 0.0 {
                                    let xr = Rect::from_center_size(img_r.right_top() + vec2(-12.0, 12.0), vec2(18.0, 18.0));
                                    let xresp = ui.interact(xr, resp.id.with("forget"), Sense::click());
                                    ui.painter().circle_filled(xr.center(), 9.0, Color32::from_black_alpha(190));
                                    ui.painter().text(xr.center(), Align2::CENTER_CENTER, "×", FontId::proportional(13.0), TEXT);
                                    if xresp.clicked() {
                                        forget = Some(r.asset.local.clone());
                                        continue;
                                    }
                                }
                                if resp.on_hover_text(r.asset.origin.location()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    open_recent = Some(idx);
                                }
                            }
                        });
                        ui.add_space(6.0);
                    }
                }
            });
        if modal.should_close() {
            self.open_sheet.visible = false;
        }
        if browse {
            self.open_sheet.visible = false;
            self.pick_files();
        }
        if let Some(req) = action {
            self.open_sheet.input.clear();
            self.open(vec![req]);
        }
        if let Some(i) = open_recent {
            self.open_recent(i);
        }
        if let Some(p) = forget {
            self.recents.remove(&p);
            self.recents.save();
        }
        if clear {
            self.recents.items.clear();
            self.recent_tex.clear();
            self.recents.save();
        }
    }

    // ------------------------------------------------------------ all photos

    /// Contact sheet of every photo in the session, with a filter — the
    /// overview that keeps large sessions manageable.
    pub fn photos_overview_ui(&mut self, ctx: &egui::Context) {
        if !self.overview.visible {
            return;
        }
        let mut switch = None;
        let mut remove = None;
        let mut open_more = false;
        let busy: std::collections::HashSet<u64> = self
            .jobs
            .iter()
            .filter(|j| j.shared.lock().unwrap().status == crate::ai::JobStatus::Running)
            .filter_map(|j| self.job_photo.get(&j.id).copied())
            .collect();
        let frame = egui::Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(16))
            .stroke(Stroke::new(1.0, LINE))
            .inner_margin(Margin::same(22))
            .shadow(egui::epaint::Shadow { offset: [0, 18], blur: 50, spread: 0, color: Color32::from_black_alpha(170) });
        let modal = egui::Modal::new(egui::Id::new("photos-overview")).frame(frame).backdrop_color(Color32::from_black_alpha(150)).show(
            ctx,
            |ui| {
                let w = 820.0;
                ui.set_width(w);
                ui.horizontal(|ui| {
                    ui.label(theme::title_italic("All photos", 28.0));
                    ui.label(RichText::new(format!("{} in this session", self.tray.len())).color(MUTED).size(12.5));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("+  Open more").clicked() {
                            open_more = true;
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.overview.filter)
                                .hint_text("Filter by name or source…")
                                .desired_width(220.0)
                                .margin(Margin::symmetric(10, 6)),
                        );
                    });
                });
                ui.add_space(12.0);
                let needle = self.overview.filter.trim().to_lowercase();
                let items: Vec<_> = self
                    .tray
                    .iter()
                    .filter(|t| {
                        needle.is_empty()
                            || t.asset.name.to_lowercase().contains(&needle)
                            || t.asset.origin.label().to_lowercase().contains(&needle)
                    })
                    .map(|t| {
                        let pieces = t.session.as_ref().map(|s| s.artworks.len()).unwrap_or(if Some(t.id) == self.current {
                            self.artworks.len()
                        } else {
                            0
                        });
                        (t.id, t.thumb.clone(), t.asset.clone(), pieces)
                    })
                    .collect();
                if items.is_empty() {
                    ui.label(RichText::new("No photos match.").color(FAINT));
                }
                let cols = 5;
                let cw = (w - 12.0 * (cols as f32 - 1.0)) / cols as f32;
                let ch = cw * 0.7;
                egui::ScrollArea::vertical().max_height(560.0).auto_shrink([false, true]).show(ui, |ui| {
                    for chunk in items.chunks(cols) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            for (id, tex, asset, pieces) in chunk {
                                let (rect, resp) = ui.allocate_exact_size(vec2(cw, ch + 38.0), Sense::click());
                                let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
                                let img = Rect::from_min_size(rect.min, vec2(cw, ch));
                                egui::Image::new(tex)
                                    .uv(cover_uv(tex.size(), cw, ch))
                                    .corner_radius(CornerRadius::same(8))
                                    .paint_at(ui, img);
                                let cur = self.current == Some(*id);
                                let p = ui.painter();
                                let stroke =
                                    if cur { Stroke::new(2.0, GOLD) } else { Stroke::new(1.0 + hov, theme::lerp_color(LINE, GOLD, hov)) };
                                p.rect_stroke(img, CornerRadius::same(8), stroke, egui::StrokeKind::Inside);
                                if cur {
                                    let b = Rect::from_min_size(img.left_top() + vec2(6.0, 6.0), vec2(64.0, 16.0));
                                    p.rect_filled(b, CornerRadius::same(8), Color32::from_black_alpha(200));
                                    p.text(b.center(), Align2::CENTER_CENTER, "ON STAGE", FontId::proportional(9.0), GOLD);
                                }
                                if *pieces > 0 {
                                    let b = Rect::from_min_size(img.right_top() + vec2(-26.0, 6.0), vec2(20.0, 16.0));
                                    p.rect_filled(b, CornerRadius::same(8), Color32::from_black_alpha(210));
                                    p.text(b.center(), Align2::CENTER_CENTER, pieces.to_string(), FontId::proportional(10.0), GOLD);
                                }
                                if busy.contains(id) {
                                    p.circle_filled(img.right_bottom() + vec2(-10.0, -10.0), 4.0, CODEX);
                                }
                                let name: String = asset.name.chars().take(26).collect();
                                p.text(
                                    pos2(rect.left() + 2.0, img.bottom() + 6.0),
                                    Align2::LEFT_TOP,
                                    name,
                                    FontId::proportional(11.5),
                                    TEXT,
                                );
                                p.text(
                                    pos2(rect.left() + 2.0, img.bottom() + 22.0),
                                    Align2::LEFT_TOP,
                                    format!("{} {}", asset.origin.icon(), asset.origin.label()),
                                    FontId::proportional(10.0),
                                    FAINT,
                                );
                                if hov > 0.0 {
                                    let xr = Rect::from_center_size(img.right_top() + vec2(-12.0, 34.0), vec2(18.0, 18.0));
                                    let xresp =
                                        ui.interact(xr, resp.id.with("remove"), Sense::click()).on_hover_text("Remove from this session");
                                    ui.painter().circle_filled(xr.center(), 9.0, Color32::from_black_alpha(190));
                                    ui.painter().text(xr.center(), Align2::CENTER_CENTER, "×", FontId::proportional(13.0), TEXT);
                                    if xresp.clicked() {
                                        remove = Some(*id);
                                        continue;
                                    }
                                }
                                if resp.on_hover_text(asset.origin.location()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    switch = Some(*id);
                                }
                            }
                        });
                        ui.add_space(8.0);
                    }
                });
            },
        );
        if modal.should_close() {
            self.overview.visible = false;
        }
        if let Some(id) = remove {
            self.remove_from_tray(id);
        }
        if let Some(id) = switch {
            self.overview.visible = false;
            self.switch_to(id);
        }
        if open_more {
            self.overview.visible = false;
            self.show_open_sheet();
        }
    }

    // ------------------------------------------------------------ progress

    /// Download / developing progress pill with a Cancel button.
    pub fn loading_pill(&mut self, ui: &mut Ui, center: egui::Pos2) {
        let Some(l) = &self.loading else { return };
        let t = ui.input(|i| i.time) as f32;
        let frac = l.total.filter(|&t| t > 0).map(|tot| (l.done as f32 / tot as f32).clamp(0.0, 1.0));
        let bytes = match (l.done, l.total) {
            (0, _) => String::new(),
            (d, Some(tot)) => format!("  ·  {} / {}", mb(d), mb(tot)),
            (d, None) => format!("  ·  {}", mb(d)),
        };
        let text = format!("{}{}", l.stage, bytes);
        let painter = ui.painter().clone();
        let g = painter.layout_no_wrap(text, FontId::proportional(12.5), TEXT);
        let w = g.size().x + 140.0;
        let pill = Rect::from_center_size(center, vec2(w, 40.0));
        painter.rect_filled(pill, CornerRadius::same(20), Color32::from_black_alpha(215));
        painter.rect_stroke(pill, CornerRadius::same(20), Stroke::new(1.0, CODEX.gamma_multiply(0.5)), egui::StrokeKind::Inside);
        let c = pos2(pill.left() + 22.0, pill.center().y);
        painter.circle_stroke(c, 9.0, Stroke::new(2.5, CODEX.gamma_multiply(0.2)));
        match frac {
            Some(f) => {
                let n = (60.0 * f) as usize;
                let pts: Vec<_> = (0..=n.max(1))
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 60.0 * std::f32::consts::TAU;
                        c + vec2(a.cos(), a.sin()) * 9.0
                    })
                    .collect();
                painter.add(egui::Shape::line(pts, Stroke::new(2.5, CODEX)));
            }
            None => {
                let a = t * 4.0;
                painter.circle_filled(c + vec2(a.cos(), a.sin()) * 9.0, 2.5, CODEX);
            }
        }
        painter.galley(pos2(pill.left() + 40.0, pill.center().y - g.size().y / 2.0), g, TEXT);
        let br = Rect::from_min_size(pos2(pill.right() - 86.0, pill.center().y - 12.0), vec2(74.0, 24.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
        if child.add(egui::Button::new(RichText::new("Cancel").size(11.5)).corner_radius(CornerRadius::same(12))).clicked() {
            self.cancel_loading();
        }
    }
}
