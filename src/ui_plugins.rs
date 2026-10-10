//! The Plug-ins panel: where plug-ins are looked for, what was found, and
//! approving or revoking them (BL-001, D7 and D9).

use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Stroke};

use crate::app::App;
use crate::plugins::{
    self,
    external::{Found, Status},
};
use crate::theme::{self, *};

enum Action {
    Approve(std::path::PathBuf, String),
    Revoke(std::path::PathBuf),
    Rescan,
    OpenUserFolder,
}

impl App {
    pub fn plugins_panel_ui(&mut self, ctx: &egui::Context) {
        if !self.plugins_open {
            return;
        }
        let mut action = None;
        let frame = egui::Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(16))
            .stroke(Stroke::new(1.0, LINE))
            .inner_margin(Margin::same(22))
            .shadow(egui::epaint::Shadow { offset: [0, 18], blur: 50, spread: 0, color: Color32::from_black_alpha(170) });
        let builtin = plugins::Registry::builtin_ids().len();
        let modal =
            egui::Modal::new(egui::Id::new("plugins-panel")).frame(frame).backdrop_color(Color32::from_black_alpha(150)).show(ctx, |ui| {
                let w = 760.0;
                ui.set_width(w);
                ui.horizontal(|ui| {
                    ui.label(theme::title_italic("Plug-ins", 28.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.scanning_plugins { "Scanning…" } else { "⟳ Rescan" };
                        if ui.add_enabled(!self.scanning_plugins, egui::Button::new(label)).clicked() {
                            action = Some(Action::Rescan);
                        }
                        if ui.button("Open my plug-ins folder").on_hover_text("Creates the folder if it doesn't exist yet").clicked() {
                            action = Some(Action::OpenUserFolder);
                        }
                    });
                });
                ui.label(
                RichText::new(format!(
                    "Extra painting styles beyond the {builtin} built-in ones. A plug-in is a program that runs on your computer with your \
                     permissions, so it only runs after you approve it, and you approve it again whenever it changes. Only approve \
                     plug-ins from people you trust."
                ))
                .color(MUTED)
                .size(12.0),
            );
                ui.add_space(12.0);

                let Some(scan) = &self.plugin_scan else {
                    ui.label(RichText::new("Looking for plug-ins…").color(FAINT));
                    return;
                };

                ui.label(theme::label_caps("Where plug-ins are looked for"));
                ui.add_space(2.0);
                for loc in &scan.locations {
                    ui.horizontal(|ui| {
                        let (dot, color) = if loc.exists { ("●", CODEX) } else { ("○", FAINT) };
                        ui.label(RichText::new(dot).color(color));
                        ui.label(RichText::new(loc.kind.label()).size(12.0).strong());
                        ui.label(RichText::new(loc.path.display().to_string()).size(11.0).color(MUTED).monospace());
                        if !loc.exists {
                            ui.label(RichText::new("not created yet").size(11.0).color(FAINT));
                        }
                    });
                }
                ui.add_space(12.0);

                ui.label(theme::label_caps("Found"));
                ui.add_space(2.0);
                if scan.found.is_empty() {
                    ui.label(
                        RichText::new(
                            "No plug-ins yet. Put a plug-in folder (one containing a plugin.toml) into your plug-ins folder, then Rescan. \
                         docs/plugins.md explains how to write one.",
                        )
                        .size(12.0)
                        .color(FAINT),
                    );
                }
                // Explicit height: inside a modal the scroll area can't infer one.
                // Cards are about 92 px tall, plus a line when the manifest has a blurb.
                let card_h = |f: &Found| if f.manifest.as_ref().is_some_and(|m| !m.blurb.is_empty()) { 114.0 } else { 92.0 };
                let list_h = scan.found.iter().map(card_h).sum::<f32>().clamp(0.0, 400.0);
                egui::ScrollArea::vertical().min_scrolled_height(list_h).max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                    for f in &scan.found {
                        egui::Frame::new()
                            .fill(CARD)
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::same(10))
                            .stroke(Stroke::new(1.0, LINE))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(&f.name).strong().size(13.5));
                                            if let Some(m) = &f.manifest {
                                                ui.label(
                                                    RichText::new(format!("{} · v{} · {} · {}", m.id, m.version, m.author, m.family))
                                                        .size(11.0)
                                                        .color(FAINT),
                                                );
                                            }
                                        });
                                        if let Some(m) = f.manifest.as_ref().filter(|m| !m.blurb.is_empty()) {
                                            ui.label(RichText::new(&m.blurb).size(11.5));
                                        }
                                        ui.label(
                                            RichText::new(format!("{} · {}", f.location.label(), f.dir.display()))
                                                .size(11.0)
                                                .color(MUTED)
                                                .monospace(),
                                        );
                                        let (text, color) = match &f.status {
                                            Status::Ready => ("Ready: available in the style gallery".to_string(), CODEX),
                                            Status::NeedsApproval => ("Needs your approval: it has not been run".to_string(), GOLD),
                                            Status::Changed => ("Changed since you approved it: approve again to use it".to_string(), GOLD),
                                            Status::Shadowed(by) => {
                                                (format!("Not used: a plug-in with the same id takes precedence ({})", by.display()), FAINT)
                                            }
                                            Status::Error(e) => (format!("Error: {e}"), DANGER),
                                        };
                                        ui.label(RichText::new(text).size(11.5).color(color));
                                    });
                                    // A fixed-size action area: inside a scroll area the available height is unbounded.
                                    let room = (ui.available_width() - 120.0).max(0.0);
                                    ui.add_space(room);
                                    ui.allocate_ui_with_layout(egui::vec2(116.0, 36.0), Layout::right_to_left(Align::Center), |ui| {
                                        let approved = f.approved;
                                        match (&f.status, &f.checksum) {
                                            (Status::NeedsApproval | Status::Changed, Some(sum)) => {
                                                if theme::accent_button(ui, "Approve", GOLD, true)
                                                    .on_hover_text(format!("Allow this plug-in to run.\nChecksum (SHA-256): {sum}"))
                                                    .clicked()
                                                {
                                                    action = Some(Action::Approve(f.dir.clone(), sum.clone()));
                                                }
                                            }
                                            _ if approved
                                                && ui.button("Revoke").on_hover_text("Stop this plug-in from running").clicked() =>
                                            {
                                                action = Some(Action::Revoke(f.dir.clone()));
                                            }
                                            _ => {}
                                        }
                                    });
                                });
                            });
                        ui.add_space(6.0);
                    }
                });
            });
        if modal.should_close() {
            self.plugins_open = false;
        }

        match action {
            Some(Action::Approve(dir, sum)) => self.approve_plugin(&dir, &sum),
            Some(Action::Revoke(dir)) => self.revoke_plugin(&dir),
            Some(Action::Rescan) => self.rescan_plugins(),
            Some(Action::OpenUserFolder) => self.open_user_plugins_folder(),
            None => {}
        }
    }
}
