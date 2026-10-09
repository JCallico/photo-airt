//! Chrome around the stage: top bar, style gallery, AI studio, atelier
//! (parameters + finish), filmstrip, toasts and drag-and-drop.

use egui::{Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::ai::{self, JobStatus};
use crate::app::{App, Kind, LeftTab, View};
use crate::styles::{Family, STYLES};
use crate::theme::{self, *};
use crate::ui_canvas::kind_color;

enum FilmAction {
    Select(Option<u64>),
    Pin(u64),
    Delete(u64),
    Export(u64),
}

fn cover_uv(tex_size: [usize; 2], w: f32, h: f32) -> Rect {
    let ta = tex_size[0] as f32 / tex_size[1] as f32;
    let ca = w / h;
    if ta > ca {
        let u0 = (1.0 - ca / ta) / 2.0;
        Rect::from_min_max(pos2(u0, 0.0), pos2(1.0 - u0, 1.0))
    } else {
        let v0 = (1.0 - ta / ca) / 2.0;
        Rect::from_min_max(pos2(0.0, v0), pos2(1.0, 1.0 - v0))
    }
}

fn shimmer(ui: &Ui, rect: Rect, rounding: CornerRadius, accent: Color32) {
    let p = ui.painter();
    p.rect_filled(rect, rounding, CARD_HI);
    let t = ui.input(|i| i.time) as f32;
    let x = rect.left() + ((t * 0.6).fract() * 1.6 - 0.3) * rect.width();
    let band = Rect::from_min_max(pos2(x - 30.0, rect.top()), pos2(x + 30.0, rect.bottom())).intersect(rect);
    if band.width() > 0.0 {
        p.rect_filled(band, CornerRadius::ZERO, accent.gamma_multiply(0.08));
    }
}

fn fmt_elapsed(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

impl App {
    pub fn shortcuts(&mut self, ctx: &egui::Context) {
        // Dropped files (several open together); browsers may hand over links.
        let dropped: Vec<_> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_string_lossy().to_string()).collect());
        let reqs: Vec<_> = dropped
            .iter()
            .filter(|p| !p.is_empty())
            .filter_map(|p| crate::sources::classify(p).ok())
            .map(crate::app_open::OpenRequest::Input)
            .collect();
        self.open(reqs);
        if self.open_sheet.visible || ctx.egui_wants_keyboard_input() {
            return;
        }
        // Ctrl+V with a link or path on the clipboard opens it straight away.
        let pasted: Vec<String> =
            ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }).collect());
        if let Some(text) = pasted.last() {
            self.open_text(text);
        }
        use egui::Key;
        if ctx.input(|i| !i.modifiers.command && i.key_pressed(Key::A)) {
            self.left_tab = if self.left_tab == LeftTab::Algorithms { LeftTab::AiStudio } else { LeftTab::Algorithms };
        }
        let (open, save, v1, v2, v3, v4, left, right, pin, reroll) = ctx.input(|i| {
            let c = i.modifiers.command;
            (
                c && i.key_pressed(Key::O),
                c && i.key_pressed(Key::S),
                i.key_pressed(Key::Num1),
                i.key_pressed(Key::Num2),
                i.key_pressed(Key::Num3),
                i.key_pressed(Key::Num4),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                !c && i.key_pressed(Key::K),
                !c && i.key_pressed(Key::R),
            )
        });
        let (prev_photo, next_photo) = ctx.input(|i| (i.key_pressed(Key::OpenBracket), i.key_pressed(Key::CloseBracket)));
        if prev_photo {
            self.tray_step(-1);
        }
        if next_photo {
            self.tray_step(1);
        }
        if open {
            self.show_open_sheet();
        }
        if save {
            self.export_selected();
        }
        for (k, v) in [(v1, View::Split), (v2, View::SideBySide), (v3, View::Single), (v4, View::Gallery)] {
            if k {
                self.view = v;
            }
        }
        if left {
            self.navigate(-1);
        }
        if right {
            self.navigate(1);
        }
        if pin {
            self.pin_draft();
        }
        if ctx.input(|i| !i.modifiers.command && i.key_pressed(Key::P)) && self.tray.len() > 1 {
            self.overview.visible = true;
        }
        if reroll && self.photo.is_some() {
            self.reroll();
        }
    }

    fn reroll(&mut self) {
        self.seed = (self.now() * 1000.0) as u64 ^ self.seed.rotate_left(17);
        self.selected = self.draft_id().or(self.selected);
        self.render_due = Some(self.now());
    }

    pub fn draw(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top")
            .exact_size(56.0)
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(16, 0)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("film")
            .exact_size(152.0)
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(14, 8)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| self.collection_bar(ui));
        egui::Panel::left("left")
            .default_size(340.0)
            .size_range(290.0..=480.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::same(14)))
            .show(ui, |ui| self.left_panel(ui));
        egui::Panel::right("right")
            .default_size(300.0)
            .size_range(260.0..=420.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::same(14)))
            .show(ui, |ui| self.right_panel(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.draw_canvas(ui));
        self.open_sheet_ui(&ctx);
        self.photos_overview_ui(&ctx);
        self.toasts_ui(&ctx);
        self.drop_overlay(&ctx);
    }

    // ------------------------------------------------------------ top bar

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
            let p = ui.painter();
            // Logo: three overlapping pigment dots.
            p.circle_filled(r.center() + vec2(-5.0, 3.0), 7.0, GOLD.gamma_multiply(0.9));
            p.circle_filled(r.center() + vec2(5.0, 3.0), 7.0, CODEX.gamma_multiply(0.8));
            p.circle_filled(r.center() + vec2(0.0, -5.0), 7.0, CLAUDE.gamma_multiply(0.8));
            ui.label(theme::title_italic("Photo·AIrt", 24.0));
            ui.label(RichText::new("studio").size(11.0).color(FAINT));
            ui.add_space(14.0);
            if ui.button("📂  Open").on_hover_text("Open from files, a link or the clipboard (Ctrl+O)").clicked() {
                self.show_open_sheet();
            }
            if let Some(ph) = &self.photo {
                let o = &ph.asset.origin;
                theme::pill(ui, MUTED, &format!("{} {}", o.icon(), o.label())).on_hover_text(o.location());
                ui.label(RichText::new(ph.name.chars().take(48).collect::<String>()).color(TEXT).size(12.5));
                ui.label(RichText::new(format!("{} × {}", ph.orig_size[0], ph.orig_size[1])).color(FAINT).size(11.5));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let can_export = self.selected_artwork().is_some();
                let label = if self.exporting { "Exporting…" } else { "Export" };
                let mut child = ui.new_child(
                    egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap()).layout(Layout::right_to_left(Align::Center)),
                );
                child.set_width(130.0);
                if theme::accent_button(&mut child, label, GOLD, can_export && !self.exporting)
                    .on_hover_text("Save the selected artwork at full resolution to ~/Pictures/Photo-AIrt (Ctrl+S)")
                    .clicked()
                {
                    self.export_selected();
                }
                ui.add_space(child.min_rect().width() + 6.0);
                if ui.button("🗁").on_hover_text("Open the output folder").clicked() {
                    self.open_output_folder();
                }
                ui.add_space(10.0);
                let status = |s: &Option<ai::CliStatus>, name: &str| match s {
                    None => (FAINT, format!("{name} …")),
                    Some(c) if c.found => (Color32::from_rgb(110, 210, 120), format!("{name} {}", c.version)),
                    Some(_) => (DANGER, format!("{name} not found")),
                };
                let (dc, tc) = status(&self.codex, "Codex");
                theme::pill(ui, dc, &tc).on_hover_text("Codex CLI — generative repainting via its image generation tool");
                let (dc, tc) = status(&self.claude, "Claude");
                theme::pill(ui, dc, &tc).on_hover_text("Claude Code CLI — art direction, vector art, wall labels");
                ui.add_space(14.0);
                // View switcher (right-to-left, so reversed).
                for (v, name, key) in [
                    (View::Gallery, "Gallery", "4"),
                    (View::Single, "Single", "3"),
                    (View::SideBySide, "Side by side", "2"),
                    (View::Split, "Split", "1"),
                ] {
                    if theme::chip(ui, name, self.view == v, GOLD).on_hover_text(format!("Key {key}")).clicked() {
                        self.view = v;
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------ left

    fn left_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 8.0) / 2.0;
            for (tab, name, accent) in [(LeftTab::Algorithms, "🎨  Algorithms", GOLD), (LeftTab::AiStudio, "✨  AI Studio", CLAUDE)] {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
                let sel = self.left_tab == tab;
                let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
                let fill = if sel { accent.gamma_multiply(0.22) } else { theme::lerp_color(CARD, CARD_HI, hov) };
                ui.painter().rect(
                    r,
                    CornerRadius::same(9),
                    fill,
                    Stroke::new(1.0, if sel { accent } else { LINE }),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    name,
                    FontId::proportional(13.5),
                    if sel { Color32::WHITE } else { MUTED },
                );
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.left_tab = tab;
                }
            }
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.left_tab {
            LeftTab::Algorithms => self.styles_gallery(ui),
            LeftTab::AiStudio => self.ai_studio(ui),
        });
    }

    fn styles_gallery(&mut self, ui: &mut Ui) {
        if self.photo.is_none() {
            ui.add_space(6.0);
            ui.label(RichText::new("Open a photo to see it in every style.").color(MUTED));
        }
        for fam in Family::ALL {
            ui.add_space(6.0);
            ui.label(theme::label_caps(fam.label()));
            ui.add_space(2.0);
            let idxs: Vec<usize> = (0..STYLES.len()).filter(|&i| STYLES[i].family == fam).collect();
            let w = ((ui.available_width() - 10.0) / 2.0).floor();
            for chunk in idxs.chunks(2) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for &i in chunk {
                        if self.style_card(ui, i, w).clicked() && self.photo.is_some() {
                            self.select_style(i);
                        }
                    }
                });
                ui.add_space(4.0);
            }
        }
    }

    fn style_card(&mut self, ui: &mut Ui, idx: usize, w: f32) -> egui::Response {
        let ih = (w * 0.7).round();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, ih + 28.0), Sense::click());
        let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
        let sel = idx == self.style_idx && self.selected_artwork().is_some_and(|a| a.kind == Kind::Algorithm);
        let lift = vec2(0.0, -2.0 * hov);
        let rect = rect.translate(lift);
        let p = ui.painter().clone();
        if hov > 0.0 || sel {
            p.add(
                egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: Color32::from_black_alpha((140.0 * hov.max(sel as u8 as f32)) as u8),
                }
                .as_shape(rect, 10),
            );
        }
        p.rect_filled(rect, CornerRadius::same(10), CARD);
        let img_r = Rect::from_min_size(rect.min, vec2(w, ih));
        let top_round = CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 };
        match &self.thumbs[idx] {
            Some(tex) => {
                egui::Image::new(tex).uv(cover_uv(tex.size(), w, ih)).corner_radius(top_round).paint_at(ui, img_r);
            }
            None => shimmer(ui, img_r, top_round, GOLD),
        }
        p.text(
            pos2(rect.left() + 10.0, rect.bottom() - 14.0),
            Align2::LEFT_CENTER,
            STYLES[idx].name,
            FontId::proportional(12.5),
            if sel { Color32::WHITE } else { TEXT },
        );
        let stroke =
            if sel { Stroke::new(2.0, GOLD) } else { Stroke::new(1.0, theme::lerp_color(LINE, Color32::from_rgb(110, 100, 110), hov)) };
        p.rect_stroke(rect, CornerRadius::same(10), stroke, egui::StrokeKind::Inside);
        if self.render.as_ref().is_some_and(|r| r.style == idx) {
            let c = pos2(img_r.right() - 16.0, img_r.top() + 16.0);
            p.circle_filled(c, 12.0, Color32::from_black_alpha(170));
            let a = ui.input(|i| i.time) as f32 * 5.0;
            p.circle_filled(c + vec2(a.cos(), a.sin()) * 6.0, 2.5, GOLD);
        }
        resp.on_hover_text(STYLES[idx].blurb).on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    // ------------------------------------------------------------ AI studio

    fn ai_studio(&mut self, ui: &mut Ui) {
        let has_photo = self.photo.is_some();
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Your Claude and Codex subscriptions, used through their CLIs. Only the photo is sent, and only to the model you pick.",
            )
            .size(11.5)
            .color(MUTED),
        );
        ui.add_space(8.0);
        self.roles_card(ui);
        ui.add_space(10.0);

        let director = self.director_role.clone();
        let painter = self.painter_role.clone();
        let d_name = director.as_ref().map(|d| d.cli.name()).unwrap_or("Director");
        let d_color = director.as_ref().map(|d| theme::engine_color(d.engine())).unwrap_or(FAINT);
        let p_name = painter.as_ref().map(|p| p.cli.name()).unwrap_or("Master Painter");
        let p_color = painter.as_ref().map(|p| theme::engine_color(p.engine())).unwrap_or(FAINT);

        // ---- Art director
        theme::card_frame(Some(d_color)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(theme::title("Art Director", 19.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::pill(ui, d_color, &director.as_ref().map(|d| d.describe()).unwrap_or("unassigned".into()));
                });
            });
            ui.label(RichText::new(format!("{d_name} studies your photo, names it, reads its palette and writes three tuned recipes for the algorithms, plus a brief for the Master Painter.")).size(12.0).color(MUTED));
            ui.add_space(4.0);
            let busy = self.ai_busy("Art direction");
            let txt = if busy { "Studying your photo…" } else if self.director.is_some() { "Direct again" } else { "✨  Direct my photo" };
            if theme::accent_button(ui, txt, d_color, director.is_some() && has_photo && !busy).clicked() {
                self.run_director();
            }
            let mut apply = None;
            let mut paint = None;
            if let Some(d) = &self.director {
                ui.add_space(8.0);
                ui.label(theme::title_italic(&d.report.title, 20.0));
                ui.label(RichText::new(&d.report.reading).size(12.0));
                ui.horizontal(|ui| {
                    for hx in &d.report.palette {
                        if let Ok(v) = u32::from_str_radix(hx.trim_start_matches('#'), 16) {
                            let c = Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
                            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                            ui.painter().circle(r.center(), 13.0, c, Stroke::new(1.5, Color32::from_white_alpha(40)));
                            resp.on_hover_text(hx);
                        }
                    }
                });
                ui.add_space(4.0);
                for (i, r) in d.report.recipes.iter().enumerate() {
                    egui::Frame::new().fill(CARD_HI).corner_radius(CornerRadius::same(9)).inner_margin(Margin::same(8)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (tr, _) = ui.allocate_exact_size(vec2(84.0, 63.0), Sense::hover());
                            match d.thumbs.get(i).and_then(|t| t.as_ref()) {
                                Some(t) => {
                                    egui::Image::new(t).uv(cover_uv(t.size(), 84.0, 63.0)).corner_radius(CornerRadius::same(6)).paint_at(ui, tr);
                                }
                                None => shimmer(ui, tr, CornerRadius::same(6), d_color),
                            }
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&r.name).strong());
                                let sname = crate::styles::style_index(&r.style).map(|s| STYLES[s].name).unwrap_or(&r.style);
                                ui.label(RichText::new(sname).size(11.0).color(GOLD));
                                ui.label(RichText::new(&r.why).size(11.0).color(MUTED));
                                if ui.small_button("Apply recipe").clicked() {
                                    apply = Some(i);
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
                if !d.report.paint_prompt.is_empty() {
                    ui.add_space(4.0);
                    ui.label(theme::label_caps(&format!("Repaint brief · {}", d.report.paint_medium)));
                    ui.label(RichText::new(&d.report.paint_prompt).italics().size(11.5).color(TEXT));
                    let busy = self.ai_busy_prefix("Repaint");
                    if theme::accent_button(ui, &format!("🎨  Paint this with {p_name}"), p_color, painter.is_some() && !busy).clicked() {
                        let m = if d.report.paint_medium.is_empty() { "Art direction".to_string() } else { d.report.paint_medium.clone() };
                        paint = Some((m, d.report.paint_prompt.clone()));
                    }
                }
            }
            if let Some(i) = apply {
                self.apply_recipe(i);
            }
            if let Some((m, b)) = paint {
                self.run_repaint(m, b, false);
            }
        });
        ui.add_space(10.0);

        // ---- Master Painter
        theme::card_frame(Some(p_color)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(theme::title("Master Painter", 19.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::pill(ui, p_color, &painter.as_ref().map(|p| p.describe()).unwrap_or("unassigned".into()));
                });
            });
            let blurb = match painter.as_ref().map(|p| p.cli) {
                Some(ai::Cli::Codex) => "Codex repaints the whole photo with its image-generation model, keeping your composition.",
                Some(ai::Cli::Claude) => "Claude repaints the photo as an SVG painting, layering hundreds of vector brush strokes. Claude Code has no raster image model.",
                None => "No installed CLI can repaint right now. Install Codex, or Claude Code for SVG paintings.",
            };
            ui.label(RichText::new(blurb).size(12.0).color(MUTED));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (i, p) in ai::PAINT_PRESETS.iter().enumerate() {
                    if theme::chip(ui, p.name, self.paint_preset == i, p_color).clicked() {
                        self.paint_preset = i;
                        self.paint_brief = p.prompt.to_string();
                    }
                }
            });
            ui.add_space(4.0);
            ui.add(egui::TextEdit::multiline(&mut self.paint_brief).desired_rows(3).desired_width(f32::INFINITY).hint_text("Describe the medium…"));
            ui.add_enabled_ui(director.is_some(), |ui| {
                ui.checkbox(&mut self.duet, RichText::new(format!("Duet: {d_name} writes a photo-specific brief first")).size(12.0))
                    .on_hover_text("The Art Director looks at your photo and expands the style into a detailed prompt, then the Master Painter paints it.");
            });
            let busy = self.ai_busy_prefix("Repaint");
            let duet = self.duet && director.is_some();
            let label = if busy { format!("{p_name} is painting…") } else { format!("🎨  Paint with {p_name}") };
            if theme::accent_button(ui, &label, if duet { DUET } else { p_color }, painter.is_some() && has_photo && !busy && !self.paint_brief.trim().is_empty()).clicked() {
                let name = ai::PAINT_PRESETS[self.paint_preset].name.to_string();
                let custom = self.paint_brief.trim() != ai::PAINT_PRESETS[self.paint_preset].prompt;
                self.run_repaint(if custom { format!("{name} (custom)") } else { name }, self.paint_brief.clone(), duet);
            }
            let hint = match painter.as_ref().map(|p| p.cli) {
                Some(ai::Cli::Codex) => "Takes about 1–2 minutes. Results are also saved to ~/Pictures/Photo-AIrt.",
                Some(ai::Cli::Claude) => "Takes about 1–3 minutes. Results are also saved to ~/Pictures/Photo-AIrt.",
                None => "",
            };
            ui.label(RichText::new(hint).size(11.0).color(FAINT));
        });
        ui.add_space(10.0);

        // ---- Vector reinterpretation (an Art Director task)
        theme::card_frame(Some(d_color)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(theme::title("Vector Reinterpretation", 19.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::pill(ui, d_color, d_name);
                });
            });
            ui.label(
                RichText::new(format!(
                    "{d_name} hand-writes an SVG artwork of your scene, shape by shape, which is rendered here with resvg."
                ))
                .size(12.0)
                .color(MUTED),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (i, v) in ai::VECTOR_STYLES.iter().enumerate() {
                    if theme::chip(ui, v.name, self.vector_style == i, d_color).on_hover_text(v.brief).clicked() {
                        self.vector_style = i;
                    }
                }
            });
            ui.add_space(4.0);
            let busy = self.ai_busy_prefix("Vector");
            let label = if busy { format!("{d_name} is composing…") } else { format!("✂  Compose with {d_name}") };
            if theme::accent_button(ui, &label, d_color, director.is_some() && has_photo && !busy).clicked() {
                self.run_vector();
            }
        });
        ui.add_space(10.0);
        self.jobs_ui(ui);
    }

    /// Assign each role to an installed CLI and pick its model.
    fn roles_card(&mut self, ui: &mut Ui) {
        theme::card_frame(None).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(theme::label_caps("Roles"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let scanning = self.claude.is_none() || self.codex.is_none();
                    if ui
                        .add_enabled(
                            !scanning,
                            egui::Button::new(RichText::new(if scanning { "Scanning…" } else { "⟳ Rescan" }).size(11.5)),
                        )
                        .on_hover_text("Re-detect installed CLIs, image generation support and models")
                        .clicked()
                    {
                        self.rescan_clis();
                    }
                });
            });
            for role in [ai::Role::Director, ai::Role::Painter] {
                ui.add_space(4.0);
                ui.label(RichText::new(role.label()).strong().size(13.0));
                let current = self.role(role).cloned();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for cli in ai::Cli::ALL {
                        let eligible = role.eligible(cli, self.cli_status(cli));
                        let selected = current.as_ref().is_some_and(|c| c.cli == cli);
                        let accent = theme::engine_color(match cli {
                            ai::Cli::Claude => ai::Engine::Claude,
                            ai::Cli::Codex => ai::Engine::Codex,
                        });
                        let resp = theme::chip_enabled(ui, cli.name(), selected, accent, eligible.is_ok());
                        let resp = match &eligible {
                            Err(why) => resp.on_hover_text(why),
                            Ok(()) if role == ai::Role::Painter && cli == ai::Cli::Claude => {
                                resp.on_hover_text("Claude has no raster image model; it repaints as SVG")
                            }
                            Ok(()) => resp,
                        };
                        if resp.clicked() && eligible.is_ok() && !selected {
                            self.set_role(role, ai::RoleCfg { cli, model: String::new() });
                        }
                    }
                    if let Some(cfg) = current.clone() {
                        self.model_picker(ui, role, cfg);
                    }
                });
                if current.is_none() && self.claude.is_some() && self.codex.is_some() {
                    ui.label(RichText::new("No installed CLI can take this role.").size(11.0).color(DANGER));
                }
            }
        });
    }

    fn model_picker(&mut self, ui: &mut Ui, role: ai::Role, cfg: ai::RoleCfg) {
        let Some(status) = self.cli_status(cfg.cli).cloned() else { return };
        let default_label = format!("Default ({})", status.default_model.as_deref().unwrap_or("CLI setting"));
        let flag = |model: &str| self.prefs.unavailable.get(&ai::RoleCfg::health_key_for(cfg.cli, model)).cloned();
        let current_label = if cfg.model.is_empty() {
            default_label.clone()
        } else {
            status.models.iter().find(|m| m.id == cfg.model).map(|m| m.label.clone()).unwrap_or(cfg.model.clone())
        };
        let warn = flag(&cfg.model);
        let shown = if warn.is_some() { format!("⚠ {current_label}") } else { current_label };
        let mut pick: Option<String> = None;
        let resp = egui::ComboBox::from_id_salt(("model", role.label()))
            .selected_text(RichText::new(shown).size(12.0))
            .width(ui.available_width().max(120.0))
            .show_ui(ui, |ui| {
                ui.set_min_width(260.0);
                let mut entry = |ui: &mut Ui, id: String, label: String, desc: String| {
                    let bad = flag(&id);
                    let text = match &bad {
                        Some(_) => RichText::new(format!("⚠ {label}")).color(DANGER),
                        None => RichText::new(label),
                    };
                    let hover = match bad {
                        Some(r) => format!("Last attempt failed: {r}"),
                        None => desc,
                    };
                    if ui.selectable_label(cfg.model == id, text).on_hover_text(hover).clicked() {
                        pick = Some(id);
                    }
                };
                entry(ui, String::new(), default_label.clone(), "Whatever the CLI is configured to use".into());
                for m in &status.models {
                    entry(ui, m.id.clone(), format!("{}  ·  {}", m.label, m.id), m.description.clone());
                }
            });
        if let Some(w) = warn {
            resp.response.on_hover_text(format!("Last attempt failed: {w}"));
        }
        if let Some(model) = pick {
            self.set_role(role, ai::RoleCfg { cli: cfg.cli, model });
        }
    }

    pub fn ai_busy_prefix(&self, prefix: &str) -> bool {
        self.jobs.iter().any(|j| j.title.starts_with(prefix) && j.shared.lock().unwrap().status == JobStatus::Running)
    }

    fn jobs_ui(&mut self, ui: &mut Ui) {
        if self.jobs.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(theme::label_caps("Studio log"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Clear finished").clicked() {
                    self.jobs.retain(|j| j.shared.lock().unwrap().status == JobStatus::Running);
                }
            });
        });
        let mut cancel = None;
        for job in self.jobs.iter().rev() {
            let s = job.shared.lock().unwrap();
            let color = theme::engine_color(job.engine);
            let elapsed = s.ended.unwrap_or_else(std::time::Instant::now).duration_since(job.started).as_secs();
            egui::Frame::new()
                .fill(CARD)
                .corner_radius(CornerRadius::same(9))
                .inner_margin(Margin::same(9))
                .stroke(Stroke::new(1.0, LINE))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, color);
                        ui.label(RichText::new(&job.title).strong().size(12.5));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (txt, c) = match &s.status {
                                JobStatus::Running => (fmt_elapsed(elapsed), color),
                                JobStatus::Done => (format!("done · {}", fmt_elapsed(elapsed)), Color32::from_rgb(110, 210, 120)),
                                JobStatus::Failed(_) => ("failed".into(), DANGER),
                                JobStatus::Cancelled => ("cancelled".into(), FAINT),
                            };
                            ui.label(RichText::new(txt).size(11.0).color(c));
                        });
                    });
                    if s.status == JobStatus::Running {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
                        ui.painter().rect_filled(r, CornerRadius::same(2), CARD_HI);
                        let t = ui.input(|i| i.time) as f32;
                        let x = r.left() + (t * 0.45).fract() * (r.width() + 80.0) - 80.0;
                        let seg = Rect::from_min_max(pos2(x.max(r.left()), r.top()), pos2((x + 80.0).min(r.right()), r.bottom()));
                        if seg.width() > 0.0 {
                            ui.painter().rect_filled(seg, CornerRadius::same(2), color);
                        }
                        ui.label(RichText::new(&s.stage).size(11.5).color(TEXT));
                    }
                    for line in s.log.iter().rev().take(3).rev() {
                        ui.label(RichText::new(line).size(10.5).color(FAINT));
                    }
                    if let JobStatus::Failed(e) = &s.status {
                        ui.label(RichText::new(e).size(11.0).color(DANGER));
                    }
                    if s.status == JobStatus::Running && ui.small_button("Cancel").clicked() {
                        cancel = Some(job.id);
                    }
                });
            ui.add_space(4.0);
        }
        if let Some(id) = cancel
            && let Some(j) = self.jobs.iter().find(|j| j.id == id)
        {
            j.cancel();
        }
    }

    // ------------------------------------------------------------ right

    fn right_panel(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let sel_kind = self.selected_artwork().map(|a| a.kind);
            ui.label(theme::label_caps("Atelier"));
            match sel_kind {
                Some(Kind::Ai(engine)) => self.ai_details(ui, engine),
                _ => self.style_controls(ui),
            }
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(4.0);
            self.finish_controls(ui);
            ui.add_space(12.0);
            ui.label(theme::label_caps("Shortcuts"));
            for (k, d) in [
                ("Space", "hold to see the original"),
                ("1 – 4", "split · side by side · single · gallery"),
                ("← →", "browse the collection"),
                ("[ ]  ·  P", "previous · next photo · all photos"),
                ("Ctrl+O · Ctrl+V", "open anywhere · paste a link"),
                ("K / R", "keep draft · reroll seed"),
                ("A", "algorithms ⇄ AI studio"),
                ("Scroll / drag", "zoom · pan · move the split"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k).size(11.0).color(GOLD).monospace());
                    ui.label(RichText::new(d).size(11.0).color(FAINT));
                });
            }
        });
    }

    fn style_controls(&mut self, ui: &mut Ui) {
        let idx = self.style_idx;
        let st = &STYLES[idx];
        ui.label(theme::title(st.name, 25.0));
        ui.label(RichText::new(st.blurb).color(MUTED).size(12.5));
        egui::CollapsingHeader::new(RichText::new("How it works").size(12.0).color(GOLD)).id_salt("how").show(ui, |ui| {
            ui.label(RichText::new(st.technique).italics().size(11.5).color(MUTED));
        });
        ui.add_space(4.0);
        let mut changed = false;
        let slider_w = ui.available_width() - 64.0;
        for spec in st.params {
            ui.label(RichText::new(spec.label).size(12.0).color(MUTED));
            let v = self.params[idx].0.get_mut(spec.key).expect("param");
            ui.spacing_mut().slider_width = slider_w;
            let mut s = egui::Slider::new(v, spec.min..=spec.max);
            if spec.step >= 1.0 {
                s = s.step_by(spec.step as f64).fixed_decimals(0);
            } else {
                s = s.step_by(spec.step as f64);
            }
            changed |= ui.add(s).changed();
        }
        if changed {
            self.params_changed();
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("🎲  Reroll").on_hover_text("New random seed (R)").clicked() && self.photo.is_some() {
                self.reroll();
            }
            if ui.button("↺  Defaults").clicked() {
                self.params[idx] = crate::styles::Params::defaults(st);
                self.params_changed();
            }
            ui.label(RichText::new(format!("seed {}", self.seed % 100000)).size(11.0).color(FAINT));
        });
        if self.draft_id().is_some() {
            ui.add_space(6.0);
            if theme::accent_button(ui, "📌  Keep in collection", GOLD, true)
                .on_hover_text("Pin this draft so the next render starts a new one (K)")
                .clicked()
            {
                self.pin_draft();
            }
        }
    }

    fn ai_details(&mut self, ui: &mut Ui, engine: ai::Engine) {
        let Some(a) = self.selected_artwork() else { return };
        ui.label(theme::title(&a.title, 25.0));
        ui.label(RichText::new(&a.subtitle).color(theme::engine_color(engine)).size(12.5));
        if let Some(p) = &a.prompt {
            ui.add_space(6.0);
            ui.label(theme::label_caps("Brief"));
            egui::Frame::new().fill(CARD).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.label(RichText::new(p).italics().size(11.5));
            });
        }
        if let Some(path) = &a.saved {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Saved to {}", path.display())).size(10.5).color(FAINT));
        }
        ui.add_space(6.0);
        if ui.button("🎨  Back to algorithms").clicked() {
            self.selected = self.draft_id();
            self.left_tab = LeftTab::Algorithms;
        }
    }

    fn finish_controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(theme::label_caps("Finish"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.selected_artwork().is_some() && ui.small_button("Reset").clicked() {
                    if let Some(a) = self.selected_artwork_mut() {
                        a.finish = Default::default();
                    }
                    self.refinish_selected();
                }
            });
        });
        let w = ui.available_width() - 64.0;
        let mut changed = false;
        match self.selected_artwork_mut() {
            Some(a) => {
                for (spec, v) in a.finish.fields_mut() {
                    ui.label(RichText::new(spec.label).size(12.0).color(MUTED));
                    ui.spacing_mut().slider_width = w;
                    changed |= ui.add(egui::Slider::new(v, spec.min..=spec.max).step_by(0.01)).changed();
                }
            }
            None => {
                ui.label(
                    RichText::new(
                        "Grade, vignette, grain, canvas weave and glow — for any artwork, algorithmic or AI. Select one to begin.",
                    )
                    .size(11.5)
                    .color(FAINT),
                );
            }
        }
        if changed {
            self.refinish_selected();
        }
    }

    // ------------------------------------------------------------ filmstrip

    /// The collection bar: one row for the whole session. The photo on stage
    /// is an expanded group (original, artworks, AI jobs in flight); every
    /// other photo is a compact stack. It scrolls to keep the current group
    /// in view, and with many photos an "All photos" grid gives an overview.
    fn collection_bar(&mut self, ui: &mut Ui) {
        if self.photo.is_none() && self.tray.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Your collection will appear here.").color(FAINT));
            });
            return;
        }
        const HEADER: f32 = 18.0;
        let th = (ui.available_height() - HEADER - 14.0).max(40.0);
        let focus = std::mem::take(&mut self.film_focus);
        let running = |j: &crate::ai::Job| {
            j.shared.lock().unwrap().status == JobStatus::Running && j.title != "Gallery placard" && j.title != "Art direction"
        };
        let busy_photos: std::collections::HashSet<u64> =
            self.jobs.iter().filter(|j| running(j)).filter_map(|j| self.job_photo.get(&j.id).copied()).collect();
        let current_jobs: Vec<(String, crate::ai::Engine, u64)> = self
            .jobs
            .iter()
            .filter(|j| running(j) && self.job_photo.get(&j.id).copied() == self.current)
            .map(|j| (j.title.clone(), j.engine, j.started.elapsed().as_secs()))
            .collect();
        // Snapshot the stacks so drawing doesn't borrow `self`.
        let stacks: Vec<(u64, egui::TextureHandle, String, String, usize)> = self
            .tray
            .iter()
            .map(|t| {
                let pieces = t.session.as_ref().map(|s| s.artworks.len()).unwrap_or(0);
                (t.id, t.thumb.clone(), t.asset.name.clone(), t.asset.origin.icon().to_string(), pieces)
            })
            .collect();
        let many = stacks.len() >= 2;
        let loading = self.loading.as_ref().map(|l| l.stage.clone());

        let mut action = None;
        let mut switch = None;
        let mut remove = None;
        let mut open_more = false;
        let mut overview = false;

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            // Pinned outside the scrolling row so it is always visible.
            if many {
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.allocate_ui(vec2(64.0, HEADER), |ui| ui.label(theme::label_caps("Photos")));
                    ui.add_space(2.0);
                    let (r, resp) = ui.allocate_exact_size(vec2(64.0, th), Sense::click());
                    let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
                    let p = ui.painter();
                    p.rect(
                        r,
                        CornerRadius::same(8),
                        theme::lerp_color(CARD, CARD_HI, hov),
                        Stroke::new(1.0, theme::lerp_color(LINE, GOLD, hov)),
                        egui::StrokeKind::Inside,
                    );
                    p.text(r.center() - vec2(0.0, 12.0), Align2::CENTER_CENTER, "⊞", FontId::proportional(24.0), GOLD);
                    p.text(
                        r.center() + vec2(0.0, 14.0),
                        Align2::CENTER_CENTER,
                        format!("All {}", stacks.len()),
                        FontId::proportional(11.0),
                        TEXT,
                    );
                    if resp.on_hover_text("See every photo in this session").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        overview = true;
                    }
                });
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, ui.available_height()), Sense::hover());
                ui.painter()
                    .line_segment([sep.center_top() + vec2(0.0, 8.0), sep.center_bottom() - vec2(0.0, 4.0)], Stroke::new(1.0, LINE));
            }
            egui::ScrollArea::horizontal().id_salt("collection").auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for (id, tex, name, icon, pieces) in &stacks {
                        if Some(*id) == self.current {
                            let group = self.current_group(ui, th, HEADER, &current_jobs, &mut action);
                            if focus {
                                group.scroll_to_me(Some(egui::Align::Center));
                            }
                        } else {
                            let r = stack_tile(ui, tex, name, icon, *pieces, busy_photos.contains(id), th, HEADER);
                            if r.clicked() {
                                switch = Some(*id);
                            }
                            r.context_menu(|ui| {
                                if ui.button("Remove from this session").clicked() {
                                    remove = Some(*id);
                                }
                            });
                        }
                    }
                    if self.current.is_none() && self.photo.is_some() {
                        self.current_group(ui, th, HEADER, &current_jobs, &mut action);
                    }
                    // Photos still arriving.
                    if let Some(stage) = &loading {
                        ui.vertical(|ui| {
                            ui.add_space(4.0);
                            ui.allocate_ui(vec2(th, HEADER), |ui| ui.label(RichText::new("Arriving").size(11.0).color(FAINT)));
                            ui.add_space(2.0);
                            let (r, _) = ui.allocate_exact_size(vec2(th, th), Sense::hover());
                            shimmer(ui, r, CornerRadius::same(8), CODEX);
                            let g = ui.painter().layout(stage.clone(), FontId::proportional(10.5), MUTED, th - 12.0);
                            ui.painter().galley(r.center() - g.size() / 2.0, g, MUTED);
                        });
                    }
                    ui.vertical(|ui| {
                        ui.add_space(4.0 + HEADER + 2.0);
                        let (r, resp) = ui.allocate_exact_size(vec2(th * 0.62, th), Sense::click());
                        let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
                        ui.painter().rect(
                            r,
                            CornerRadius::same(8),
                            theme::lerp_color(CARD, CARD_HI, hov),
                            Stroke::new(1.0, theme::lerp_color(LINE, GOLD, hov)),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(r.center() - vec2(0.0, 8.0), Align2::CENTER_CENTER, "+", FontId::proportional(22.0), MUTED);
                        ui.painter().text(r.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, "Add", FontId::proportional(10.5), FAINT);
                        if resp.on_hover_text("Open more photos (Ctrl+O)").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            open_more = true;
                        }
                    });
                });
            });
        });

        match action {
            Some(FilmAction::Select(id)) => self.select(id),
            Some(FilmAction::Pin(id)) => {
                if let Some(a) = self.artworks.iter_mut().find(|a| a.id == id) {
                    a.pinned = true;
                }
            }
            Some(FilmAction::Delete(id)) => self.delete_artwork(id),
            Some(FilmAction::Export(id)) => {
                self.selected = Some(id);
                self.export_selected();
            }
            None => {}
        }
        if let Some(id) = switch {
            self.switch_to(id);
        }
        if let Some(id) = remove {
            self.remove_from_tray(id);
        }
        if open_more {
            self.show_open_sheet();
        }
        if overview {
            self.overview.visible = true;
        }
    }

    /// The photo on stage, expanded: a framed group with its original,
    /// artworks and AI jobs in flight.
    fn current_group(
        &self,
        ui: &mut Ui,
        th: f32,
        header: f32,
        jobs: &[(String, crate::ai::Engine, u64)],
        action: &mut Option<FilmAction>,
    ) -> egui::Response {
        let Some(photo) = &self.photo else { return ui.label("") };
        let tile_h = th - 6.0;
        let aspect = photo.work.w as f32 / photo.work.h as f32;
        let tw = (tile_h * aspect).clamp(tile_h * 0.6, tile_h * 1.8);
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(27, 25, 31))
            .stroke(Stroke::new(1.0, GOLD.gamma_multiply(0.4)))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin { left: 8, right: 8, top: 4, bottom: 6 });
        frame
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), header), Layout::left_to_right(Align::Center), |ui| {
                        let o = &photo.asset.origin;
                        ui.label(
                            RichText::new(format!("{} {}", o.icon(), photo.name.chars().take(48).collect::<String>()))
                                .size(11.5)
                                .color(TEXT)
                                .strong(),
                        )
                        .on_hover_text(o.location());
                        let n = self.artworks.len();
                        if n > 0 {
                            ui.label(RichText::new(format!("· {n} piece{}", if n == 1 { "" } else { "s" })).size(11.0).color(FAINT));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if self.film_tile(ui, &photo.tex, "Original", MUTED, self.selected.is_none(), false, tw, tile_h).clicked() {
                            *action = Some(FilmAction::Select(None));
                        }
                        for a in &self.artworks {
                            let r = self.film_tile(
                                ui,
                                &a.tex,
                                &a.title,
                                kind_color(a.kind),
                                self.selected == Some(a.id),
                                !a.pinned,
                                tw,
                                tile_h,
                            );
                            if r.clicked() {
                                *action = Some(FilmAction::Select(Some(a.id)));
                            }
                            r.context_menu(|ui| {
                                if !a.pinned && ui.button("📌 Keep in collection").clicked() {
                                    *action = Some(FilmAction::Pin(a.id));
                                }
                                if ui.button("Export…").clicked() {
                                    *action = Some(FilmAction::Export(a.id));
                                }
                                if ui.button("🗑 Remove").clicked() {
                                    *action = Some(FilmAction::Delete(a.id));
                                }
                            });
                        }
                        for (title, engine, secs) in jobs {
                            let (r, _) = ui.allocate_exact_size(vec2(tw, tile_h), Sense::hover());
                            let color = theme::engine_color(*engine);
                            shimmer(ui, r, CornerRadius::same(8), color);
                            ui.painter().rect_stroke(
                                r,
                                CornerRadius::same(8),
                                Stroke::new(1.0, color.gamma_multiply(0.6)),
                                egui::StrokeKind::Inside,
                            );
                            let g = ui.painter().layout(title.clone(), FontId::proportional(11.0), TEXT, tw - 12.0);
                            ui.painter().galley(r.center() - vec2(g.size().x / 2.0, g.size().y / 2.0 + 8.0), g, TEXT);
                            ui.painter().text(
                                r.center() + vec2(0.0, 12.0),
                                Align2::CENTER_CENTER,
                                fmt_elapsed(*secs),
                                FontId::proportional(11.0),
                                color,
                            );
                        }
                    });
                });
            })
            .response
    }

    #[allow(clippy::too_many_arguments)]
    fn film_tile(
        &self,
        ui: &mut Ui,
        tex: &egui::TextureHandle,
        title: &str,
        dot: Color32,
        selected: bool,
        draft: bool,
        w: f32,
        h: f32,
    ) -> egui::Response {
        let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
        egui::Image::new(tex).uv(cover_uv(tex.size(), w, h)).corner_radius(CornerRadius::same(8)).paint_at(ui, r);
        let p = ui.painter();
        // Bottom gradient for the caption.
        let cap = Rect::from_min_max(pos2(r.left(), r.bottom() - 26.0), r.max);
        let mut m = egui::Mesh::default();
        m.colored_vertex(cap.left_top(), Color32::TRANSPARENT);
        m.colored_vertex(cap.right_top(), Color32::TRANSPARENT);
        m.colored_vertex(cap.left_bottom(), Color32::from_black_alpha(210));
        m.colored_vertex(cap.right_bottom(), Color32::from_black_alpha(210));
        m.add_triangle(0, 1, 2);
        m.add_triangle(1, 2, 3);
        p.with_clip_rect(r.shrink(1.0)).add(egui::Shape::mesh(m));
        p.circle_filled(pos2(r.left() + 10.0, r.bottom() - 10.0), 3.0, dot);
        let font = FontId::proportional(11.0);
        let g = p.layout(title.to_string(), font, TEXT, w - 26.0);
        p.with_clip_rect(r).galley(pos2(r.left() + 18.0, r.bottom() - 10.0 - g.size().y / 2.0), g, TEXT);
        if draft {
            let b = Rect::from_min_size(pos2(r.right() - 44.0, r.top() + 6.0), vec2(38.0, 16.0));
            p.rect_filled(b, CornerRadius::same(8), Color32::from_black_alpha(180));
            p.text(b.center(), Align2::CENTER_CENTER, "DRAFT", FontId::proportional(9.0), GOLD);
        }
        let stroke = if selected { Stroke::new(2.0, GOLD) } else { Stroke::new(1.0, Color32::from_white_alpha((20.0 + 60.0 * hov) as u8)) };
        p.rect_stroke(r, CornerRadius::same(8), stroke, egui::StrokeKind::Inside);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(title)
    }

    // ------------------------------------------------------------ overlays

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let now = self.now();
        egui::Area::new(egui::Id::new("toasts")).anchor(Align2::RIGHT_BOTTOM, vec2(-330.0, -140.0)).interactable(false).show(ctx, |ui| {
            for t in &self.toasts {
                let age = (now - t.born) as f32;
                let alpha = (age / 0.2).min(1.0) * ((5.0 - age) / 0.6).clamp(0.0, 1.0);
                egui::Frame::new()
                    .fill(CARD.gamma_multiply(alpha))
                    .corner_radius(CornerRadius::same(10))
                    .stroke(Stroke::new(1.0, t.color.gamma_multiply(0.7 * alpha)))
                    .inner_margin(Margin::symmetric(12, 9))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 6],
                        blur: 18,
                        spread: 0,
                        color: Color32::from_black_alpha((120.0 * alpha) as u8),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(380.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, t.color.gamma_multiply(alpha));
                            ui.label(RichText::new(&t.text).size(12.5).color(TEXT.gamma_multiply(alpha)));
                        });
                    });
                ui.add_space(6.0);
            }
        });
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        if !ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            return;
        }
        let rect = ctx.content_rect();
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
        p.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(190));
        p.rect_stroke(rect.shrink(30.0), CornerRadius::same(24), Stroke::new(2.0, GOLD), egui::StrokeKind::Inside);
        p.text(rect.center(), Align2::CENTER_CENTER, "Drop to open your photo", FontId::new(36.0, serif_italic()), TEXT);
    }
}

/// A parked photo as a compact stack: offset cards behind the thumbnail, a
/// name above, a badge counting its artworks and a pulse while AI works on it.
#[allow(clippy::too_many_arguments)]
fn stack_tile(
    ui: &mut Ui,
    tex: &egui::TextureHandle,
    name: &str,
    icon: &str,
    pieces: usize,
    busy: bool,
    th: f32,
    header: f32,
) -> egui::Response {
    let size = tex.size();
    let w = (th * size[0] as f32 / size[1].max(1) as f32).clamp(th * 0.62, th * 0.95);
    let inner = ui.vertical(|ui| {
        ui.add_space(4.0);
        let chars = ((w + 6.0) / 6.5) as usize;
        let label: String = if name.chars().count() > chars {
            format!("{}…", name.chars().take(chars.saturating_sub(1)).collect::<String>())
        } else {
            name.to_string()
        };
        ui.allocate_ui(vec2(w + 6.0, header), |ui| ui.label(RichText::new(label).size(11.0).color(MUTED)));
        ui.add_space(2.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w + 6.0, th), Sense::click());
        let hov = ui.ctx().animate_bool(resp.id, resp.hovered());
        let front = Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(w, th - 6.0));
        let p = ui.painter();
        for (k, shade) in [(2.0f32, 34u8), (1.0, 46)] {
            let back = front.translate(vec2(3.0 * k, -3.0 * k));
            p.rect(
                back,
                CornerRadius::same(7),
                Color32::from_gray(shade),
                Stroke::new(1.0, Color32::from_white_alpha(18)),
                egui::StrokeKind::Inside,
            );
        }
        egui::Image::new(tex).uv(cover_uv(size, front.width(), front.height())).corner_radius(CornerRadius::same(7)).paint_at(ui, front);
        let p = ui.painter();
        p.rect_filled(front, CornerRadius::same(7), Color32::from_black_alpha((110.0 * (1.0 - hov)) as u8));
        p.rect_stroke(
            front,
            CornerRadius::same(7),
            Stroke::new(1.0, theme::lerp_color(Color32::from_white_alpha(30), GOLD, hov)),
            egui::StrokeKind::Inside,
        );
        p.text(front.left_bottom() + vec2(6.0, -5.0), Align2::LEFT_BOTTOM, icon, FontId::proportional(10.0), TEXT);
        if pieces > 0 {
            let b = Rect::from_min_size(front.right_top() + vec2(-24.0, 5.0), vec2(19.0, 15.0));
            p.rect_filled(b, CornerRadius::same(7), Color32::from_black_alpha(210));
            p.text(b.center(), Align2::CENTER_CENTER, pieces.to_string(), FontId::proportional(9.5), GOLD);
        }
        if busy {
            let t = ui.input(|i| i.time) as f32;
            let c = front.right_bottom() + vec2(-9.0, -9.0);
            p.circle_filled(c, 4.0 + 1.5 * (t * 4.0).sin().abs(), CODEX.gamma_multiply(0.5));
            p.circle_filled(c, 3.0, CODEX);
        }
        let tip = if pieces > 0 {
            format!("{name}\n{pieces} piece{} · click to open", if pieces == 1 { "" } else { "s" })
        } else {
            format!("{name}\nclick to open")
        };
        resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
    });
    inner.inner
}
