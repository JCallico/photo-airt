//! The stage: split / side-by-side / single comparison with zoom & pan,
//! a "paint sweep" reveal for new results, the framed gallery view, and the
//! animated empty state.

use egui::{Align2, Color32, CornerRadius, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, TextureId, Ui, Vec2, pos2, vec2};

use crate::app::{App, Kind, View};
use crate::styles::STYLES;
use crate::theme::{self, *};

const UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

pub fn fit(avail: Rect, w: f32, h: f32) -> Rect {
    let s = (avail.width() / w).min(avail.height() / h).max(0.01);
    Rect::from_center_size(avail.center(), vec2(w * s, h * s))
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn vgradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), top);
    m.colored_vertex(rect.right_top(), top);
    m.colored_vertex(rect.left_bottom(), bottom);
    m.colored_vertex(rect.right_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 2, 3);
    painter.add(Shape::mesh(m));
}

/// Radial glow as a triangle fan fading to transparent.
fn radial(painter: &egui::Painter, center: Pos2, radii: Vec2, color: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(center, color);
    let n = 64;
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        m.colored_vertex(center + vec2(a.cos() * radii.x, a.sin() * radii.y), Color32::TRANSPARENT);
    }
    for i in 0..n {
        m.add_triangle(0, i + 1, i + 2);
    }
    painter.add(Shape::mesh(m));
}

fn ring(painter: &egui::Painter, c: Pos2, r: f32, frac: f32, color: Color32, width: f32) {
    painter.circle_stroke(c, r, Stroke::new(width, color.gamma_multiply(0.18)));
    let n = 60;
    let pts: Vec<Pos2> = (0..=((n as f32 * frac.clamp(0.0, 1.0)) as usize))
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    if pts.len() > 1 {
        painter.add(Shape::line(pts, Stroke::new(width, color)));
    }
}

fn label(painter: &egui::Painter, anchor: Pos2, align: Align2, text: &str, dot: Option<Color32>) {
    let font = FontId::proportional(11.5);
    let g = painter.layout_no_wrap(text.to_string(), font, TEXT);
    let pad = vec2(10.0, 5.0);
    let extra = if dot.is_some() { 12.0 } else { 0.0 };
    let size = g.size() + pad * 2.0 + vec2(extra, 0.0);
    let r = align.anchor_size(anchor, size);
    painter.rect_filled(r, CornerRadius::same(12), Color32::from_black_alpha(165));
    if let Some(c) = dot {
        painter.circle_filled(pos2(r.left() + 12.0, r.center().y), 3.5, c);
    }
    painter.galley(r.min + pad + vec2(extra, 0.0), g, TEXT);
}

pub fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Algorithm => GOLD,
        Kind::Ai(e) => theme::engine_color(e),
    }
}

impl App {
    pub fn draw_canvas(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let now = self.now();
        let t = now as f32;
        if let Some(rv) = &self.reveal
            && now - rv.start > 1.0
        {
            self.reveal = None;
        }

        vgradient(&painter, rect, Color32::from_rgb(17, 16, 20), Color32::from_rgb(9, 8, 11));

        if self.photo.is_none() {
            self.draw_empty(ui, &painter, rect, t);
            return;
        }
        if self.view == View::Gallery {
            self.draw_gallery(ui, &painter, rect, t);
            return;
        }

        let photo = self.photo.as_ref().unwrap();
        let (iw, ih) = (photo.work.w as f32, photo.work.h as f32);
        let photo_tex = photo.tex.id();
        let hold = ui.input(|i| i.key_down(egui::Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
        let art = self.selected_artwork().map(|a| (a.id, a.tex.id(), a.title.clone(), a.kind));
        let avail = Rect::from_min_max(rect.min + vec2(28.0, 46.0), rect.max - vec2(28.0, 28.0));

        // Ambient glow behind the image.
        radial(&painter, avail.center(), avail.size() * 0.62, Color32::from_rgba_unmultiplied(227, 177, 92, 10));

        if self.view == View::SideBySide && art.is_some() && !hold {
            let (_, tex, title, kind) = art.clone().unwrap();
            let gap = 18.0;
            let half = (avail.width() - gap) / 2.0;
            let l = fit(Rect::from_min_size(avail.min, vec2(half, avail.height())), iw, ih);
            let r = fit(Rect::from_min_size(avail.min + vec2(half + gap, 0.0), vec2(half, avail.height())), iw, ih);
            for (rr, tid) in [(l, photo_tex), (r, tex)] {
                painter.add(
                    egui::epaint::Shadow { offset: [0, 14], blur: 36, spread: 0, color: Color32::from_black_alpha(150) }.as_shape(rr, 4),
                );
                painter.image(tid, rr, UV, Color32::WHITE);
            }
            self.draw_reveal_overlay(&painter, r, r, now);
            label(&painter, l.left_top() + vec2(10.0, 10.0), Align2::LEFT_TOP, "Original", Some(MUTED));
            label(&painter, r.left_top() + vec2(10.0, 10.0), Align2::LEFT_TOP, &title, Some(kind_color(kind)));
        } else {
            let base = fit(avail, iw, ih);
            let img_rect = Rect::from_center_size(base.center() + self.pan, base.size() * self.zoom);
            painter.add(
                egui::epaint::Shadow { offset: [0, 18], blur: 44, spread: 2, color: Color32::from_black_alpha(170) }.as_shape(img_rect, 4),
            );

            match (&art, hold, self.view) {
                (Some((id, tex, title, kind)), false, View::Split) => {
                    let split_x = img_rect.left() + img_rect.width() * self.split;
                    let left_clip = Rect::from_min_max(img_rect.min, pos2(split_x, img_rect.max.y));
                    let right_clip = Rect::from_min_max(pos2(split_x, img_rect.min.y), img_rect.max);
                    painter.with_clip_rect(left_clip.intersect(rect)).image(photo_tex, img_rect, UV, Color32::WHITE);
                    self.draw_art(&painter, *id, *tex, img_rect, right_clip.intersect(rect), now);
                    self.draw_handle(&painter, split_x, img_rect.intersect(rect), t);
                    let lab_y = img_rect.top().max(rect.top() + 8.0) + 10.0;
                    if self.split > 0.12 {
                        label(&painter, pos2(img_rect.left().max(rect.left()) + 10.0, lab_y), Align2::LEFT_TOP, "Original", Some(MUTED));
                    }
                    if self.split < 0.88 {
                        label(
                            &painter,
                            pos2(img_rect.right().min(rect.right()) - 10.0, lab_y),
                            Align2::RIGHT_TOP,
                            title,
                            Some(kind_color(*kind)),
                        );
                    }
                    // Handle interaction.
                    let near = resp.hover_pos().is_some_and(|p| (p.x - split_x).abs() < 18.0 && img_rect.y_range().contains(p.y));
                    if resp.drag_started() {
                        self.dragging_split = resp.interact_pointer_pos().is_some_and(|p| (p.x - split_x).abs() < 18.0);
                    }
                    if near || self.dragging_split {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                }
                (Some((id, tex, title, kind)), false, _) => {
                    self.draw_art(&painter, *id, *tex, img_rect, img_rect.intersect(rect), now);
                    label(
                        &painter,
                        pos2(img_rect.left().max(rect.left()) + 10.0, img_rect.top().max(rect.top() + 8.0) + 10.0),
                        Align2::LEFT_TOP,
                        title,
                        Some(kind_color(*kind)),
                    );
                }
                _ => {
                    painter.image(photo_tex, img_rect, UV, Color32::WHITE);
                    let txt = if hold { "Original · release Space to return" } else { "Original photograph" };
                    label(
                        &painter,
                        pos2(img_rect.left().max(rect.left()) + 10.0, img_rect.top().max(rect.top() + 8.0) + 10.0),
                        Align2::LEFT_TOP,
                        txt,
                        Some(MUTED),
                    );
                }
            }

            // Zoom & pan.
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll.abs() > 0.0 {
                    let old = self.zoom;
                    let new = (old * (scroll * 0.0018).exp()).clamp(0.2, 16.0);
                    if let Some(p) = resp.hover_pos() {
                        let c = base.center() + self.pan;
                        let c2 = p - (p - c) * (new / old);
                        self.pan = c2 - base.center();
                    }
                    self.zoom = new;
                }
            }
            if resp.dragged() {
                if self.dragging_split {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.split = ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0);
                    }
                } else {
                    self.pan += resp.drag_delta();
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            }
            if resp.drag_stopped() {
                self.dragging_split = false;
            }
            if resp.double_clicked() {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
            if (self.zoom - 1.0).abs() > 0.01 {
                label(
                    &painter,
                    rect.right_top() + vec2(-14.0, 14.0),
                    Align2::RIGHT_TOP,
                    &format!("{:.0}%  ·  double-click to fit", self.zoom * 100.0),
                    None,
                );
            }
        }

        // Render progress.
        if let Some(r) = &self.render {
            let frac = r.progress.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
            let txt = format!("Painting {} …  {:.0}%", STYLES[r.style].name, frac * 100.0);
            let g = painter.layout_no_wrap(txt, FontId::proportional(12.5), TEXT);
            let w = g.size().x + 60.0;
            let pill = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 34.0), vec2(w, 36.0));
            painter.rect_filled(pill, CornerRadius::same(18), Color32::from_black_alpha(200));
            painter.rect_stroke(pill, CornerRadius::same(18), Stroke::new(1.0, GOLD.gamma_multiply(0.5)), egui::StrokeKind::Inside);
            let c = pos2(pill.left() + 20.0, pill.center().y);
            ring(&painter, c, 9.0, frac.max(0.04), GOLD, 2.5);
            // A little orbiting spark.
            let a = t * 4.0;
            painter.circle_filled(c + vec2(a.cos(), a.sin()) * 9.0, 2.0, Color32::WHITE);
            painter.galley(pos2(pill.left() + 38.0, pill.center().y - g.size().y / 2.0), g, TEXT);
        }
    }

    fn draw_art(&self, painter: &egui::Painter, id: u64, tex: TextureId, img_rect: Rect, clip: Rect, now: f64) {
        match &self.reveal {
            Some(rv) if rv.artwork == id => {
                let t = ((now - rv.start) / 0.9).clamp(0.0, 1.0) as f32;
                let e = ease_out(t);
                if let Some(prev) = &rv.prev {
                    painter.with_clip_rect(clip).image(prev.id(), img_rect, UV, Color32::WHITE);
                } else {
                    painter.with_clip_rect(clip).image(tex, img_rect, UV, Color32::from_white_alpha((e * 255.0) as u8));
                    return;
                }
                let sweep = img_rect.left() + img_rect.width() * e;
                let c2 = clip.intersect(Rect::from_min_max(img_rect.min, pos2(sweep, img_rect.max.y)));
                painter.with_clip_rect(c2).image(tex, img_rect, UV, Color32::WHITE);
                self.draw_reveal_overlay(painter, img_rect, clip, now);
            }
            _ => {
                painter.with_clip_rect(clip).image(tex, img_rect, UV, Color32::WHITE);
            }
        }
    }

    /// The bright brush-edge that travels across a freshly painted result.
    fn draw_reveal_overlay(&self, painter: &egui::Painter, img_rect: Rect, clip: Rect, now: f64) {
        let Some(rv) = &self.reveal else { return };
        let t = ((now - rv.start) / 0.9).clamp(0.0, 1.0) as f32;
        if t >= 1.0 || rv.prev.is_none() {
            return;
        }
        let x = img_rect.left() + img_rect.width() * ease_out(t);
        let fade = 1.0 - t;
        let band = 60.0;
        let mut m = Mesh::default();
        let (top, bot) = (img_rect.top(), img_rect.bottom());
        let glow = Color32::from_rgba_unmultiplied(255, 220, 160, (110.0 * fade) as u8);
        m.colored_vertex(pos2(x - band, top), Color32::TRANSPARENT);
        m.colored_vertex(pos2(x, top), glow);
        m.colored_vertex(pos2(x - band, bot), Color32::TRANSPARENT);
        m.colored_vertex(pos2(x, bot), glow);
        m.add_triangle(0, 1, 2);
        m.add_triangle(1, 2, 3);
        painter.with_clip_rect(clip).add(Shape::mesh(m));
        painter
            .with_clip_rect(clip)
            .line_segment([pos2(x, top), pos2(x, bot)], Stroke::new(2.0, Color32::from_white_alpha((220.0 * fade) as u8)));
    }

    fn draw_handle(&self, painter: &egui::Painter, x: f32, r: Rect, t: f32) {
        painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(4.0, Color32::from_black_alpha(90)));
        painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(1.5, Color32::from_white_alpha(230)));
        let c = pos2(x, r.center().y);
        let pulse = if self.dragging_split { 1.0 } else { 0.5 + 0.5 * (t * 2.0).sin() };
        painter.circle_filled(c, 22.0, GOLD.gamma_multiply(0.10 + 0.1 * pulse));
        painter.circle_filled(c, 16.0, Color32::from_rgb(24, 22, 27));
        painter.circle_stroke(c, 16.0, Stroke::new(1.5, Color32::WHITE));
        for dir in [-1.0f32, 1.0] {
            let tip = c + vec2(dir * 9.0, 0.0);
            let pts = vec![tip, c + vec2(dir * 4.0, -4.5), c + vec2(dir * 4.0, 4.5)];
            painter.add(Shape::convex_polygon(pts, Color32::WHITE, Stroke::NONE));
        }
    }

    fn draw_empty(&mut self, ui: &mut Ui, painter: &egui::Painter, rect: Rect, t: f32) {
        let blobs = [GOLD, CLAUDE, CODEX, DUET, Color32::from_rgb(90, 140, 230), Color32::from_rgb(230, 90, 120)];
        for (k, col) in blobs.iter().enumerate() {
            let kf = k as f32 + 1.0;
            let c = rect.center()
                + vec2(
                    (t * 0.13 * kf.sqrt() + kf * 1.7).sin() * rect.width() * 0.32,
                    (t * 0.11 * (kf * 0.7).sqrt() + kf * 2.3).cos() * rect.height() * 0.28,
                );
            let r = rect.width().min(rect.height()) * (0.22 + 0.04 * (t * 0.3 + kf).sin());
            radial(painter, c, vec2(r, r), col.gamma_multiply(0.16));
        }
        // Drifting brush marks.
        for k in 0..18 {
            let kf = k as f32;
            let y = rect.top() + (kf * 53.0 + t * (8.0 + kf)) % rect.height();
            let x = rect.left() + ((kf * 137.0) % rect.width());
            let len = 30.0 + (kf * 17.0) % 60.0;
            let col = blobs[k % blobs.len()].gamma_multiply(0.18);
            painter.line_segment([pos2(x, y), pos2(x + len, y - len * 0.35)], Stroke::new(3.0 + (k % 3) as f32, col));
        }
        let c = rect.center();
        painter.text(c + vec2(0.0, -70.0), Align2::CENTER_CENTER, "Photo·AIrt", FontId::new(76.0, serif_italic()), TEXT);
        painter.text(
            c + vec2(0.0, -8.0),
            Align2::CENTER_CENTER,
            "Turn your photographs into art — with classic algorithms, Claude and Codex.",
            FontId::proportional(15.5),
            MUTED,
        );
        let btn = Rect::from_center_size(c + vec2(0.0, 52.0), vec2(230.0, 34.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(btn));
        if self.loading.is_some() {
            ring(painter, btn.center() + vec2(-80.0, 0.0), 10.0, 0.25 + 0.2 * (t * 3.0).sin(), GOLD, 2.5);
            painter.text(
                btn.center() + vec2(10.0, 0.0),
                Align2::CENTER_CENTER,
                format!("Developing {}…", self.loading.as_deref().unwrap_or("")),
                FontId::proportional(14.0),
                TEXT,
            );
        } else if theme::accent_button(&mut child, "Open a photo", GOLD, true).clicked() {
            self.pick_file();
        }
        painter.text(c + vec2(0.0, 98.0), Align2::CENTER_CENTER, "or drop an image anywhere  ·  Ctrl+O", FontId::proportional(12.0), FAINT);
    }

    fn draw_gallery(&mut self, ui: &mut Ui, painter: &egui::Painter, rect: Rect, t: f32) {
        let _ = t;
        // Wall, floor and a warm spotlight.
        let floor_y = rect.bottom() - rect.height() * 0.1;
        vgradient(
            painter,
            Rect::from_min_max(rect.min, pos2(rect.max.x, floor_y)),
            Color32::from_rgb(44, 40, 48),
            Color32::from_rgb(27, 25, 31),
        );
        vgradient(
            painter,
            Rect::from_min_max(pos2(rect.min.x, floor_y), rect.max),
            Color32::from_rgb(20, 18, 22),
            Color32::from_rgb(10, 9, 11),
        );
        painter.line_segment([pos2(rect.left(), floor_y), pos2(rect.right(), floor_y)], Stroke::new(1.0, Color32::from_white_alpha(14)));
        radial(
            painter,
            pos2(rect.center().x, rect.top() + rect.height() * 0.36),
            vec2(rect.width() * 0.45, rect.height() * 0.6),
            Color32::from_rgba_unmultiplied(255, 226, 180, 34),
        );

        let Some(photo) = &self.photo else { return };
        let (iw, ih) = (photo.work.w as f32, photo.work.h as f32);
        let art = self.selected_artwork();
        let tex = art.map(|a| a.tex.id()).unwrap_or(photo.tex.id());
        let avail = Rect::from_min_max(rect.min + vec2(80.0, 50.0), pos2(rect.max.x - 80.0, floor_y - 120.0));
        let ft = (avail.width().min(avail.height()) * 0.035).clamp(12.0, 28.0);
        let mat = ft * 1.3;
        let img = fit(avail.shrink(ft + mat), iw, ih);
        let mat_r = img.expand(mat);
        let frame = mat_r.expand(ft);

        painter
            .add(egui::epaint::Shadow { offset: [0, 26], blur: 60, spread: 6, color: Color32::from_black_alpha(190) }.as_shape(frame, 2));
        // Gilded frame: four bevelled sides.
        let (o, i) = (frame, mat_r);
        let sides = [
            ([o.left_top(), o.right_top(), i.right_top(), i.left_top()], Color32::from_rgb(201, 162, 98)),
            ([o.left_top(), i.left_top(), i.left_bottom(), o.left_bottom()], Color32::from_rgb(170, 132, 74)),
            ([i.left_bottom(), i.right_bottom(), o.right_bottom(), o.left_bottom()], Color32::from_rgb(98, 72, 36)),
            ([i.right_top(), o.right_top(), o.right_bottom(), i.right_bottom()], Color32::from_rgb(122, 92, 48)),
        ];
        for (pts, col) in sides {
            painter.add(Shape::convex_polygon(pts.to_vec(), col, Stroke::NONE));
        }
        painter.rect_stroke(
            frame.shrink(ft * 0.32),
            CornerRadius::ZERO,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 230, 170, 60)),
            egui::StrokeKind::Middle,
        );
        painter.rect_stroke(i, CornerRadius::ZERO, Stroke::new(1.5, Color32::from_rgb(70, 50, 24)), egui::StrokeKind::Outside);
        painter.rect_filled(mat_r, CornerRadius::ZERO, Color32::from_rgb(239, 233, 220));
        // Bevelled mat window.
        painter.rect_stroke(
            img.expand(3.0),
            CornerRadius::ZERO,
            Stroke::new(3.0, Color32::from_rgb(250, 247, 240)),
            egui::StrokeKind::Outside,
        );
        painter.image(tex, img, UV, Color32::WHITE);
        painter.line_segment([img.left_top(), img.right_top()], Stroke::new(2.0, Color32::from_black_alpha(60)));
        painter.line_segment([img.left_top(), img.left_bottom()], Stroke::new(2.0, Color32::from_black_alpha(40)));

        // Wall label.
        let (title, medium, note) = match art {
            Some(a) => match &a.placard {
                Some(p) => (p.title.clone(), format!("{}, {}", p.medium, p.year), p.note.clone()),
                None => {
                    let note = match a.kind {
                        Kind::Algorithm => a.recipe.as_ref().map(|r| STYLES[r.0].technique.to_string()).unwrap_or_default(),
                        Kind::Ai(_) => a.prompt.clone().unwrap_or_default().chars().take(260).collect(),
                    };
                    (a.title.clone(), a.subtitle.clone(), note)
                }
            },
            None => (photo.name.clone(), format!("Photograph, {} × {} px", photo.orig_size[0], photo.orig_size[1]), String::new()),
        };
        let pw = 300.0;
        let ink = Color32::from_rgb(32, 29, 33);
        let gt = painter.layout(title, FontId::new(18.0, serif_italic()), ink, pw - 32.0);
        let gm = painter.layout(medium, FontId::proportional(11.5), Color32::from_rgb(105, 98, 100), pw - 32.0);
        let gn = painter.layout(note, FontId::proportional(11.0), Color32::from_rgb(70, 65, 68), pw - 32.0);
        let ph = 16.0 + gt.size().y + 4.0 + gm.size().y + if gn.size().y > 1.0 { 10.0 + gn.size().y } else { 0.0 } + 16.0;
        let px = (frame.right() - pw).max(rect.left() + 20.0);
        let py = (frame.bottom() + 34.0).min(rect.bottom() - ph - 12.0);
        let pr = Rect::from_min_size(pos2(px, py), vec2(pw, ph));
        painter.add(egui::epaint::Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(120) }.as_shape(pr, 2));
        painter.rect_filled(pr, CornerRadius::same(2), Color32::from_rgb(244, 240, 231));
        let mut y = pr.top() + 16.0;
        let (gth, gmh) = (gt.size().y, gm.size().y);
        painter.galley(pos2(pr.left() + 16.0, y), gt, ink);
        y += gth + 4.0;
        painter.galley(pos2(pr.left() + 16.0, y), gm, ink);
        y += gmh + 10.0;
        painter.galley(pos2(pr.left() + 16.0, y), gn, ink);

        // Ask Claude to write the label.
        let director = self.director_role.as_ref().map(|d| (d.cli.name(), theme::engine_color(d.engine())));
        if let (Some(a), Some((who, color))) = (art, director)
            && a.placard.is_none()
        {
            let busy = self.ai_busy("Gallery placard");
            let br = Rect::from_min_size(pos2(frame.left(), pr.top()), vec2(250.0, 32.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
            let txt = if busy { format!("{who} is writing…") } else { format!("✒  Ask {who} for a wall label") };
            if theme::accent_button(&mut child, &txt, color, !busy).clicked() {
                self.run_placard();
            }
        }
    }
}
