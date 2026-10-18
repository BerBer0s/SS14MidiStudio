//! Раскладка главного окна: верхняя панель, список файлов, сводка по файлу, редактор, транспорт.

use egui::{Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use ss14midi_core::analysis::Metrics;
use ss14midi_core::{Limits, Preset, Status};

use super::{SideTab, StudioApp, reveal};
use crate::editor::NoteId;
use crate::i18n::{self, Lang, count, set_lang, tr};
use crate::library::{Entry, EntryState};
use crate::theme::{self, bold, mono, ui_font};
use crate::views::channels::{self, ChannelAction, ChannelFlags};
use crate::views::overview::{self, OverviewAction, OverviewInput, fmt_time, fmt_time_ms};
use crate::views::report::{self, ReportAction};
use crate::views::roll::Tool;
use crate::widgets::{self, Icon};

const BAR_BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x13);

impl StudioApp {
    pub(super) fn layout(&mut self, ui: &mut Ui) {
        egui::Panel::top("top")
            .exact_size(48.0)
            .frame(egui::Frame::NONE.fill(BAR_BG).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("transport")
            .exact_size(48.0)
            .frame(egui::Frame::NONE.fill(BAR_BG).inner_margin(egui::Margin::symmetric(12, 0)).stroke(Stroke::new(1.0, theme::LINE)))
            .show(ui, |ui| self.transport(ui));
        egui::Panel::left("files")
            .resizable(true)
            .default_size(290.0)
            .size_range(220.0..=480.0)
            .frame(egui::Frame::NONE.fill(theme::PANEL).stroke(Stroke::new(1.0, theme::LINE)))
            .show(ui, |ui| self.files_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::BG).inner_margin(egui::Margin { left: 14, right: 12, top: 10, bottom: 8 }))
            .show(ui, |ui| self.inspector(ui));
    }

    // ── Верхняя панель ──────────────────────────────────────────────────────

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            let (logo, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(logo, CornerRadius::same(4), theme::ACCENT);
            for (i, w) in [12.0, 8.0, 14.0].into_iter().enumerate() {
                let y = logo.top() + 6.0 + i as f32 * 4.0;
                p.rect_filled(Rect::from_min_size(Pos2::new(logo.left() + 4.0, y), Vec2::new(w, 2.0)), 0.0, theme::INK);
            }
            ui.label(RichText::new("SS14 MIDI Studio").font(bold(16.0)).color(theme::TEXT));
            ui.add_space(16.0);
            if widgets::button(ui, tr("Открыть файлы…", "Open files…")).on_hover_text("Ctrl+O").clicked() {
                self.pick_files();
            }
            if widgets::button(ui, tr("Открыть папку…", "Open folder…")).clicked() {
                self.pick_folder();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let next = if self.prefs.lang == Lang::Ru { Lang::En } else { Lang::Ru };
                if widgets::button(ui, if self.prefs.lang == Lang::Ru { "English" } else { "Русский" }).clicked() {
                    self.prefs.lang = next;
                    set_lang(next);
                }
                if widgets::button_ex(ui, tr("Справка", "Help"), true, self.show_help).on_hover_text("F1").clicked() {
                    self.show_help = !self.show_help;
                }
                if widgets::button_ex(ui, tr("Настройки", "Settings"), true, self.show_settings).clicked() {
                    self.show_settings = !self.show_settings;
                }
            });
        });
    }

    // ── Список файлов ───────────────────────────────────────────────────────

    fn files_panel(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("batch")
            .frame(egui::Frame::NONE.fill(theme::PANEL).inner_margin(egui::Margin::same(12)).stroke(Stroke::new(1.0, theme::LINE)))
            .show(ui, |ui| self.batch_box(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.file_list(ui));
    }

    fn file_list(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(tr("Файлы", "Files")).font(bold(15.0)).color(theme::TEXT));
            if !self.entries.is_empty() {
                ui.label(RichText::new(self.entries.len().to_string()).font(mono(13.0)).color(theme::TEXT_DIM));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(8.0);
                if !self.entries.is_empty() {
                    let resp = widgets::icon_button(ui, Icon::More, tr("Действия со списком", "List actions"), true);
                    egui::Popup::menu(&resp).show(|ui| {
                        if ui.button(tr("Закрыть все файлы", "Close all files")).clicked() {
                            self.close_all();
                        }
                    });
                }
            });
        });
        ui.add_space(6.0);
        if self.entries.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(tr("Пока пусто. Перетащите MIDI-файлы в окно.", "Nothing here yet. Drop MIDI files onto the window."))
                        .color(theme::TEXT_FAINT),
                );
            });
            return;
        }
        let row_h = 52.0;
        let mut clicked = None;
        let mut closed = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, self.entries.len(), |ui, range| {
            for e in &self.entries[range] {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                let selected = self.selected == Some(e.id);
                file_row(ui, rect, e, selected, resp.hovered());
                if resp.clicked() {
                    clicked = Some(e.id);
                }
                if resp.middle_clicked() {
                    closed = Some(e.id);
                }
                resp.context_menu(|ui| {
                    if ui.button(tr("Показать в проводнике", "Show in Explorer")).clicked() {
                        reveal(&e.path);
                    }
                    if ui.button(tr("Закрыть", "Close")).clicked() {
                        closed = Some(e.id);
                    }
                });
            }
        });
        if let Some(id) = clicked {
            self.select(id);
        }
        if let Some(id) = closed {
            self.remove_entry(id);
        }
    }

    fn batch_box(&mut self, ui: &mut Ui) {
        let ready = self.entries.iter().filter(|e| e.is_ready()).count();
        let busy = self.entries.iter().filter(|e| e.busy).count();
        ui.label(RichText::new(tr("Все файлы сразу", "All files at once")).font(bold(14.0)).color(theme::TEXT));
        ui.label(
            RichText::new(format!("{} «{}»", tr("Режим", "Mode"), i18n::preset_label(self.prefs.settings.preset))).font(ui_font(13.0)).color(theme::TEXT_DIM),
        );
        ui.add_space(4.0);
        let label = if busy > 0 {
            format!("{} {busy}…", tr("Обработка", "Processing"))
        } else {
            format!("{} ({ready})", tr("Обработать и сохранить все", "Process and save all"))
        };
        ui.scope(|ui| {
            ui.set_width(ui.available_width());
            if widgets::primary_button(ui, &label, ready > 0 && busy == 0)
                .on_hover_text(tr(
                    "Каждый открытый файл обрабатывается выбранным режимом и сохраняется",
                    "Every open file is processed with the selected mode and saved",
                ))
                .clicked()
            {
                self.process_all();
            }
        });
        ui.add_space(6.0);
        let where_ = match &self.prefs.out_dir {
            Some(d) => short_path(&d.display().to_string(), 34),
            None => tr("рядом с исходными файлами", "next to the source files").to_string(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new(format!("{} {where_}", tr("Сохранять:", "Save to:"))).font(ui_font(13.0)).color(theme::TEXT_DIM));
            if ui.link(RichText::new(tr("изменить", "change")).font(ui_font(13.0))).clicked() {
                self.pick_out_dir();
            }
            if self.prefs.out_dir.is_some() && ui.link(RichText::new(tr("сбросить", "reset")).font(ui_font(13.0))).clicked() {
                self.prefs.out_dir = None;
            }
        });
    }

    // ── Центральная часть ───────────────────────────────────────────────────

    fn inspector(&mut self, ui: &mut Ui) {
        let Some(id) = self.selected else {
            self.empty_state(ui);
            return;
        };
        let Some(e) = self.entry(id) else { return };
        match &e.state {
            EntryState::Loading => {
                ui.centered_and_justified(|ui| ui.label(RichText::new(tr("Загрузка…", "Loading…")).color(theme::TEXT_DIM)));
                return;
            }
            EntryState::Failed(err) => {
                let text = format!("{}\n{err}", tr("Не удалось открыть файл", "Could not open the file"));
                ui.centered_and_justified(|ui| ui.label(RichText::new(text).color(theme::OVER)));
                return;
            }
            EntryState::Ready => {}
        }
        self.file_header(ui, id);
        ui.add_space(8.0);
        self.summary(ui, id);
        ui.add_space(8.0);
        self.actions(ui, id);
        ui.add_space(10.0);
        self.overview(ui, id);
        ui.add_space(8.0);
        self.roll_toolbar(ui, id);
        ui.add_space(4.0);

        egui::Panel::right("side")
            .resizable(false)
            .exact_size(300.0)
            .frame(egui::Frame::NONE.fill(theme::BG).inner_margin(egui::Margin { left: 10, right: 0, top: 0, bottom: 0 }))
            .show(ui, |ui| self.side_column(ui, id));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.roll(ui, id));
    }

    fn empty_state(&mut self, ui: &mut Ui) {
        let avail = ui.available_rect_before_wrap();
        let rect = Rect::from_center_size(avail.center(), Vec2::new(avail.width().min(560.0), 300.0));
        let p = ui.painter();
        p.rect(rect, CornerRadius::same(8), theme::PANEL, Stroke::new(1.0, theme::LINE_STRONG), StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(28.0)).layout(Layout::top_down(Align::Center)));
        let ui = &mut child;
        ui.label(RichText::new(tr("Перетащите сюда MIDI-файлы или папку", "Drop MIDI files or a folder here")).font(bold(19.0)).color(theme::TEXT));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let w = 290.0;
            ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
            if widgets::primary_button(ui, tr("Выбрать файлы", "Choose files"), true).clicked() {
                self.pick_files();
            }
            if widgets::button(ui, tr("Выбрать папку", "Choose folder")).clicked() {
                self.pick_folder();
            }
        });
        ui.add_space(22.0);
        let steps = [
            tr("Программа проверит, уложится ли каждый файл в лимиты инструментов SS14.", "The app checks whether each file fits SS14 instrument limits."),
            tr("Выберите режим обработки и нажмите «Обработать».", "Choose a processing mode and press “Process”."),
            tr("Послушайте результат, при желании поправьте ноты и сохраните.", "Listen to the result, tweak notes if you like, and save."),
        ];
        for (i, s) in steps.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).font(bold(14.0)).color(theme::ACCENT));
                ui.label(RichText::new(*s).color(theme::TEXT_DIM));
            });
        }
    }

    fn file_header(&mut self, ui: &mut Ui, id: u64) {
        let Some(e) = self.entry(id) else { return };
        let (name, path, dirty) = (e.name(), e.path.display().to_string(), e.dirty);
        let (can_undo, can_redo) = (e.history.can_undo(), e.history.can_redo());
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).font(bold(19.0)).color(theme::TEXT)).on_hover_text(&path);
            if dirty {
                ui.label(RichText::new(tr("не сохранён", "unsaved")).font(ui_font(13.0)).color(theme::ACCENT));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = widgets::icon_button(ui, Icon::More, tr("Ещё", "More"), true);
                egui::Popup::menu(&more).show(|ui| {
                    if ui.button(tr("Сохранить как…", "Save as…")).clicked() {
                        self.save_as(id);
                    }
                    if ui.button(tr("Показать в проводнике", "Show in Explorer")).clicked()
                        && let Some(e) = self.entry(id)
                    {
                        reveal(e.saved_to.as_deref().unwrap_or(&e.path));
                    }
                    ui.separator();
                    if ui.button(tr("Вернуть исходный файл", "Revert to source file")).clicked() {
                        self.reset_to_original();
                    }
                    if ui.button(tr("Закрыть файл", "Close file")).clicked() {
                        self.remove_entry(id);
                    }
                });
                ui.add_space(4.0);
                if widgets::icon_button(ui, Icon::Redo, &format!("{} (Ctrl+Y)", tr("Повторить", "Redo")), can_redo).clicked() {
                    self.undo(true);
                }
                if widgets::icon_button(ui, Icon::Undo, &format!("{} (Ctrl+Z)", tr("Отменить", "Undo")), can_undo).clicked() {
                    self.undo(false);
                }
            });
        });
    }

    /// Вердикт по файлу и основные числа.
    fn summary(&mut self, ui: &mut Ui, id: u64) {
        let limits = self.prefs.settings.limits;
        let Some(e) = self.entry(id) else { return };
        let Some(status) = e.status() else { return };
        let now = e.analysis.as_ref().or(e.source.as_ref().map(|s| &s.analysis));
        let Some(now) = now else { return };
        let src = e.source.as_ref().map(|s| &s.analysis.metrics);
        let changed = e.report.is_some() || e.history.can_undo();
        let (title, detail) = verdict(status, &now.metrics, &limits, now.ranges.len());
        let damaged = e.source.as_ref().is_some_and(|s| !s.problems.is_empty());
        let removed = e.report.as_ref().map_or(0, |r| r.changes.removed_notes.len());
        let notes = e.working.note_count();

        let mut open_report = false;
        let card = egui::Frame::NONE
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::LINE))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin { left: 14, right: 12, top: 10, bottom: 10 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let gauges_w = 3.0 * 132.0 + 2.0 * 8.0;
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - gauges_w - 12.0).max(200.0));
                        ui.label(RichText::new(title).font(bold(16.0)).color(theme::status_color(status)));
                        ui.add(egui::Label::new(RichText::new(detail).color(theme::TEXT_DIM)).wrap());
                        if damaged {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    RichText::new(tr(
                                        "Исходный файл был повреждён и восстановлен при открытии.",
                                        "The source file was damaged and repaired on open.",
                                    ))
                                    .color(theme::WARN),
                                );
                                if ui.link(tr("Подробнее", "Details")).clicked() {
                                    open_report = true;
                                }
                            });
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let was = |v: u64, old: Option<u64>| match old.filter(|o| changed && *o != v) {
                            Some(o) => format!("{} {}", tr("было", "was"), group_digits(o)),
                            None => String::new(),
                        };
                        let m = &now.metrics;
                        let removed_text = if removed > 0 { format!("{} {removed}", tr("убрано", "removed")) } else { String::new() };
                        gauge(ui, tr("Ноты", "Notes"), group_digits(notes as u64), removed_text, None);
                        let frame = m.peak_per_frame.value;
                        gauge(
                            ui,
                            tr("Событий за кадр", "Events per frame"),
                            format!("{frame} / {}", limits.frame_limit),
                            was(frame as u64, src.map(|s| s.peak_per_frame.value as u64)),
                            Some((frame as f32 / limits.frame_limit as f32, level(frame, limits.frame_limit, limits.frame_limit))),
                        );
                        let sec = m.peak_per_second.value;
                        gauge(
                            ui,
                            tr("Событий в секунду", "Events per second"),
                            format!("{sec} / {}", limits.max_per_second),
                            was(sec as u64, src.map(|s| s.peak_per_second.value as u64)),
                            Some((sec as f32 / limits.max_per_second as f32, level(sec, limits.target_per_second, limits.max_per_second))),
                        );
                    });
                });
            })
            .response
            .rect;
        let strip = Rect::from_min_size(card.min, Vec2::new(4.0, card.height()));
        ui.painter().rect_filled(strip, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, theme::status_color(status));
        if open_report {
            self.side_tab = SideTab::Report;
        }
    }

    /// Режим обработки и главные кнопки.
    fn actions(&mut self, ui: &mut Ui, id: u64) {
        let Some(e) = self.entry(id) else { return };
        let busy = e.busy;
        let processed = e.report.is_some();
        let target = self.save_target(e).display().to_string();
        let mut process = false;
        let mut save = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Обработка", "Processing")).font(bold(14.0)).color(theme::TEXT));
            let options: Vec<(Preset, &str, &str)> = Preset::ALL.iter().map(|&p| (p, i18n::preset_label(p), "")).collect();
            widgets::segmented(ui, &mut self.prefs.settings.preset, &options);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let save_label = tr("Сохранить", "Save");
                let process_label = if busy { tr("Обработка…", "Processing…") } else { tr("Обработать", "Process") };
                // Главная кнопка — следующий шаг: сначала обработать, потом сохранить.
                let save_resp = if processed { widgets::primary_button(ui, save_label, !busy) } else { widgets::button_ex(ui, save_label, !busy, false) };
                if save_resp.on_hover_text(format!("Ctrl+S → {target}")).clicked() {
                    save = true;
                }
                let process_resp =
                    if processed { widgets::button_ex(ui, process_label, !busy, false) } else { widgets::primary_button(ui, process_label, !busy) };
                if process_resp
                    .on_hover_text(format!("Ctrl+Enter · {}", tr("применить режим к файлу; можно отменить", "apply the mode to the file; can be undone")))
                    .clicked()
                {
                    process = true;
                }
                ui.add(egui::Label::new(RichText::new(i18n::preset_hint(self.prefs.settings.preset)).font(ui_font(12.5)).color(theme::TEXT_DIM)).wrap());
            });
        });
        if process {
            self.process(id, false);
        }
        if save {
            self.save(id, None);
        }
    }

    fn overview(&mut self, ui: &mut Ui, id: u64) {
        let limits = self.prefs.settings.limits;
        let playing = self.player.is_playing();
        let pos_s = self.position_us() as f64 / 1e6;
        let (t0, t1) = self.roll.visible_ticks();
        let Some(e) = self.entry(id) else { return };
        let map = e.working.tempo_map();
        let view = (map.tick_to_sec(t0.max(0.0) as u64), map.tick_to_sec(t1.max(0.0) as u64));
        let Some(a) = e.analysis.as_ref().or(e.source.as_ref().map(|s| &s.analysis)) else { return };
        let src = e.source.as_ref().map(|s| &s.analysis);
        let changed = e.report.is_some() || e.history.can_undo();
        let duration = a.metrics.duration_s;
        let input = OverviewInput {
            timeline: &a.timeline,
            source: src.map(|s| s.timeline.as_slice()).filter(|_| changed),
            ranges: &a.ranges,
            duration_s: duration.max(src.map_or(0.0, |s| s.metrics.duration_s).min(duration * 4.0)).max(1.0),
            limits: &limits,
            view,
            playhead_s: (playing || self.cursor_us > 0).then_some(pos_s),
        };
        match overview::show(ui, &input, 80.0) {
            Some(OverviewAction::Navigate(s)) => self.roll.center_on(map.us_to_tick(s * 1e6) as f64),
            Some(OverviewAction::Seek(s)) => self.seek((s * 1e6) as u64),
            None => {}
        }
    }

    fn roll_toolbar(&mut self, ui: &mut Ui, id: u64) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            widgets::segmented(
                ui,
                &mut self.roll.tool,
                &[
                    (Tool::Select, tr("Выделение", "Select"), tr("Выделять и двигать ноты (V)", "Select and move notes (V)")),
                    (Tool::Draw, tr("Рисование", "Draw"), tr("Рисовать ноты в выбранный канал (D)", "Draw notes into the chosen channel (D)")),
                ],
            );
            ui.add_space(6.0);
            widgets::caption(ui, tr("Сетка", "Grid"));
            let snaps = [(0u32, tr("нет", "off")), (1, "1/4"), (2, "1/8"), (4, "1/16"), (8, "1/32")];
            let current = snaps.iter().find(|s| s.0 == self.roll.snap_div).map_or("1/16", |s| s.1);
            egui::ComboBox::from_id_salt("snap").width(70.0).selected_text(current).show_ui(ui, |ui| {
                for (v, l) in snaps {
                    if ui.selectable_label(self.roll.snap_div == v, l).clicked() {
                        self.roll.snap_div = v;
                        self.prefs.snap_div = v;
                    }
                }
            });
            ui.add_space(6.0);
            if widgets::check(ui, self.roll.show_ghosts, tr("Убранные ноты", "Removed notes"))
                .on_hover_text(tr(
                    "Показывать красными рамками ноты, убранные обработкой. Двойной клик по рамке возвращает ноту.",
                    "Show notes removed by processing as red outlines. Double-click an outline to restore the note.",
                ))
                .clicked()
            {
                self.roll.show_ghosts = !self.roll.show_ghosts;
                self.prefs.show_ghosts = self.roll.show_ghosts;
            }
            ui.add_space(6.0);
            if widgets::icon_button(ui, Icon::Minus, tr("Отдалить (−)", "Zoom out (−)"), true).clicked() {
                self.roll.zoom_x(0.8, self.roll.view_width / 2.0);
            }
            if widgets::icon_button(ui, Icon::Plus, tr("Приблизить (+)", "Zoom in (+)"), true).clicked() {
                self.roll.zoom_x(1.25, self.roll.view_width / 2.0);
            }
            if widgets::icon_button(ui, Icon::Fit, tr("Показать с начала (F)", "Fit from the start (F)"), true).clicked() {
                self.roll.needs_fit = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let sel = self.roll.selection.len();
                if let Some((ch, n)) = self.roll.hover {
                    let Some(e) = self.entry(id) else { return };
                    let at = fmt_time_ms(e.working.tempo_map().tick_to_sec(n.start));
                    let text =
                        format!("{} {} · {} · {} {} · {at}", tr("канал", "ch"), ch + 1, ss14midi_core::gm::note_name(n.key), tr("громк.", "vel"), n.velocity);
                    ui.label(RichText::new(text).font(mono(12.0)).color(theme::channel_color(ch)));
                } else if sel > 0 {
                    ui.label(
                        RichText::new(count(sel, ["нота выбрана", "ноты выбрано", "нот выбрано"], ["note selected", "notes selected"]))
                            .font(ui_font(13.0))
                            .color(theme::ACCENT),
                    );
                }
            });
        });
    }

    fn roll(&mut self, ui: &mut Ui, id: u64) {
        let playing = self.player.is_playing();
        let position = self.position_us();
        let Some(e) = self.entries.iter().find(|e| e.id == id) else { return };
        let map = e.working.tempo_map();
        let overloads: Vec<(u64, u64)> =
            e.analysis.as_ref().map(|a| a.ranges.iter().map(|r| (map.us_to_tick(r.from_s * 1e6), map.us_to_tick(r.to_s * 1e6))).collect()).unwrap_or_default();
        let playhead = (playing || self.cursor_us > 0).then(|| map.us_to_tick(position as f64));
        if let (true, true, Some(t)) = (playing, self.prefs.follow, playhead) {
            self.roll.follow(t as f64);
        }
        let actions = self.roll.show(ui, &e.working, &map, &e.ghosts, &overloads, playhead);
        self.apply_roll(actions);
    }

    fn side_column(&mut self, ui: &mut Ui, id: u64) {
        widgets::segmented(ui, &mut self.side_tab, &[(SideTab::Channels, tr("Каналы", "Channels"), ""), (SideTab::Report, tr("Отчёт", "Report"), "")]);
        ui.add_space(8.0);
        match self.side_tab {
            SideTab::Channels => {
                let flags = ChannelFlags { mute: self.mute, solo: self.solo, hidden: self.roll.hidden, active: self.roll.active_channel };
                let Some(e) = self.entries.iter().find(|e| e.id == id) else { return };
                for a in channels::show(ui, &e.working, &flags) {
                    match a {
                        ChannelAction::ToggleMute(c) => self.mute ^= 1 << c,
                        ChannelAction::ToggleSolo(c) => self.solo ^= 1 << c,
                        ChannelAction::ToggleHidden(c) => self.roll.hidden ^= 1 << c,
                        ChannelAction::ShowOnly(c) => self.roll.hidden = !(1u16 << c),
                        ChannelAction::ShowAll => self.roll.hidden = 0,
                        ChannelAction::SetActive(c) => {
                            self.roll.active_channel = c as u8;
                            self.roll.hidden &= !(1 << c);
                        }
                        ChannelAction::SetProgram(c, p) => self.edit(tr("Инструмент", "Instrument"), |s, _| crate::editor::set_program(s, c, p)),
                        ChannelAction::Clear(c) => self.edit(tr("Удаление канала", "Clear channel"), |s, sel| {
                            crate::editor::clear_channel(s, c);
                            sel.retain(|x: &NoteId| x.ch as usize != c);
                        }),
                    }
                }
            }
            SideTab::Report => {
                let Some(e) = self.entry(id) else { return };
                for a in report::show(ui, e) {
                    match a {
                        ReportAction::JumpSeconds(s) => {
                            let Some(e) = self.entry(id) else { return };
                            let tick = e.working.tempo_map().us_to_tick(s * 1e6);
                            self.roll.center_on(tick as f64);
                            self.seek((s * 1e6) as u64);
                        }
                        ReportAction::Restore(i) => self.restore_ghosts(&[i]),
                        ReportAction::RestoreAll => {
                            let n = self.entry(id).map_or(0, |e| e.ghosts.len());
                            self.restore_ghosts(&(0..n).collect::<Vec<_>>());
                        }
                    }
                }
            }
        }
    }

    // ── Транспорт ───────────────────────────────────────────────────────────

    fn transport(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            let playing = self.player.is_playing();
            let has_song = self.selected_entry().is_some_and(|e| e.is_ready());
            if widgets::icon_button(ui, Icon::Stop, tr("Стоп, в начало (Home)", "Stop, back to start (Home)"), has_song).clicked() {
                self.stop();
            }
            let (icon, hint) = if playing { (Icon::Pause, tr("Пауза (пробел)", "Pause (Space)")) } else { (Icon::Play, tr("Слушать (пробел)", "Play (Space)")) };
            if widgets::icon_toggle(ui, icon, hint, has_song, playing).clicked() {
                self.toggle_play();
            }
            let total = self.duration_s();
            let pos = self.position_us() as f64 / 1e6;
            ui.label(RichText::new(format!("{} / {}", fmt_time_ms(pos), fmt_time_ms(total))).font(mono(14.0)).color(if playing { theme::ACCENT } else { theme::TEXT }));

            // Справа налево: выход, режимы, затем полоса перемотки на оставшуюся ширину.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let port = self.player.shared.port.lock().unwrap().clone();
                if port.is_empty()
                    && ui.link(RichText::new(tr("Нет MIDI-выхода — настроить", "No MIDI output — set up")).color(theme::WARN)).clicked()
                {
                    self.show_settings = true;
                }
                if widgets::check(ui, self.prefs.follow, tr("Следить", "Follow")).on_hover_text(tr("Пианоролл прокручивается за воспроизведением", "The piano roll scrolls with playback")).clicked() {
                    self.prefs.follow = !self.prefs.follow;
                }
                if self.prefs.simulate {
                    let lag = self.player.shared.lag_us.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0;
                    let max = self.player.shared.max_lag_us.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0;
                    let color = if max > 250.0 { theme::OVER } else if max > 30.0 { theme::WARN } else { theme::TEXT_DIM };
                    ui.label(RichText::new(format!("{} {lag:.0} {} ({} {max:.0})", tr("отставание", "lag"), tr("мс", "ms"), tr("макс.", "max"))).font(mono(12.0)).color(color));
                }
                if widgets::check(ui, self.prefs.simulate, tr("Как услышат другие", "As others hear it"))
                    .on_hover_text(tr(
                        "Воспроизводит файл с сетевыми лимитами клиента SS14: не больше 60 событий за кадр и 1000 в секунду. Перегруженный файл начнёт отставать — так его услышат другие игроки.",
                        "Plays the file with SS14 client network limits: at most 60 events per frame and 1000 per second. An overloaded file starts to lag, which is how other players hear it.",
                    ))
                    .clicked()
                {
                    self.prefs.simulate = !self.prefs.simulate;
                    self.player.set_simulate(self.prefs.simulate);
                }
                let mut original = self.ab_original;
                if widgets::segmented(
                    ui,
                    &mut original,
                    &[
                        (true, tr("Исходник", "Source"), tr("Слушать исходный файл", "Play the source file")),
                        (false, tr("Результат", "Result"), tr("Слушать текущую версию с правками", "Play the current version with edits")),
                    ],
                ) {
                    self.set_ab(original);
                }
                ui.add_space(8.0);
                if let Some(s) = seek_bar(ui, pos, total, has_song) {
                    self.seek((s * 1e6) as u64);
                }
            });
        });
    }

    // ── Поверх всего ────────────────────────────────────────────────────────

    pub(super) fn overlays(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(190));
            let frame = screen.shrink(28.0);
            painter.rect_stroke(frame, CornerRadius::same(10), Stroke::new(2.0, theme::ACCENT), StrokeKind::Inside);
            painter.text(frame.center(), Align2::CENTER_CENTER, tr("Отпустите, чтобы добавить файлы", "Release to add the files"), bold(24.0), theme::ACCENT);
        }
        if let Some(t) = &self.toast {
            let age = ctx.input(|i| i.time) - t.at;
            if age < 5.0 {
                let alpha = (1.0 - ((age - 4.0) / 1.0).clamp(0.0, 1.0)) as f32;
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toast")));
                let galley = painter.layout(t.text.clone(), ui_font(14.0), theme::TEXT, 460.0);
                let pos = Pos2::new(screen.right() - 16.0, screen.bottom() - 60.0);
                let rect = Align2::RIGHT_BOTTOM.anchor_size(pos, galley.size() + Vec2::new(28.0, 16.0));
                painter.rect(
                    rect,
                    CornerRadius::same(6),
                    theme::RAISED.gamma_multiply(alpha),
                    Stroke::new(1.0, theme::LINE_STRONG.gamma_multiply(alpha)),
                    StrokeKind::Inside,
                );
                painter.rect_filled(
                    Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())),
                    CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 },
                    t.color.gamma_multiply(alpha),
                );
                painter.galley_with_override_text_color(rect.min + Vec2::new(16.0, 8.0), galley, theme::TEXT.gamma_multiply(alpha));
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
    }
}

fn file_row(ui: &Ui, rect: Rect, e: &Entry, selected: bool, hovered: bool) {
    let p = ui.painter();
    p.rect_filled(
        rect,
        0.0,
        if selected {
            theme::HOVER
        } else if hovered {
            theme::RAISED
        } else {
            theme::PANEL
        },
    );
    if selected {
        p.rect_filled(Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())), 0.0, theme::ACCENT);
    }
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::LINE));
    let status = e.status().filter(|_| e.is_ready());
    let pill_w = status.map_or(0.0, |s| {
        let r = widgets::status_pill(p, Pos2::new(rect.right() - 10.0, rect.top() + 9.0), Align2::RIGHT_TOP, s);
        r.width()
    });
    let name = truncate_to(p, &e.name(), ui_font(14.0), rect.width() - pill_w - 36.0);
    p.text(
        Pos2::new(rect.left() + 14.0, rect.top() + 8.0),
        Align2::LEFT_TOP,
        name,
        ui_font(14.0),
        if selected { theme::TEXT } else { theme::TEXT.gamma_multiply(0.85) },
    );
    let (info, color) = match &e.state {
        EntryState::Loading => (tr("загрузка…", "loading…").to_string(), theme::TEXT_DIM),
        EntryState::Failed(_) => (tr("не удалось открыть", "could not open").to_string(), theme::OVER),
        EntryState::Ready => {
            let m = e.analysis.as_ref().or(e.source.as_ref().map(|s| &s.analysis)).map(|a| &a.metrics);
            let text = m.map_or(String::new(), |m| {
                format!(
                    "{} · {} {} · {} {}",
                    fmt_time(m.duration_s),
                    m.peak_per_second.value,
                    tr("в сек.", "/s"),
                    m.peak_per_frame.value,
                    tr("за кадр", "/frame")
                )
            });
            (text, theme::TEXT_DIM)
        }
    };
    p.text(Pos2::new(rect.left() + 14.0, rect.top() + 30.0), Align2::LEFT_TOP, info, ui_font(12.5), color);
    let mark = if e.busy {
        Some((tr("обработка…", "processing…"), theme::ACCENT))
    } else if e.dirty {
        Some((tr("не сохранён", "unsaved"), theme::ACCENT))
    } else if e.saved_to.is_some() {
        Some((tr("сохранён", "saved"), theme::OK))
    } else {
        None
    };
    if let Some((text, color)) = mark {
        p.text(Pos2::new(rect.right() - 10.0, rect.bottom() - 8.0), Align2::RIGHT_BOTTOM, text, ui_font(12.0), color);
    }
}

fn verdict(status: Status, m: &Metrics, limits: &Limits, overloads: usize) -> (String, String) {
    let (sec, frame) = (m.peak_per_second.value, m.peak_per_frame.value);
    match status {
        Status::Ok => (
            tr("Пройдёт в SS14", "Fits SS14 limits").to_string(),
            match crate::i18n::lang() {
                Lang::Ru => format!(
                    "Пик нагрузки — {sec} событий в секунду из {} и {frame} за кадр из {}. Инструмент будет играть без задержек.",
                    limits.max_per_second, limits.frame_limit
                ),
                Lang::En => format!(
                    "Peak load is {sec} events per second of {} and {frame} per frame of {}. The instrument will play without lag.",
                    limits.max_per_second, limits.frame_limit
                ),
            },
        ),
        Status::Warn => (
            tr("На грани лимитов", "Close to the limits").to_string(),
            match crate::i18n::lang() {
                Lang::Ru => format!(
                    "Пик — {sec} событий в секунду: выше безопасной отметки {}, хотя лимит {} не превышен. У части игроков возможны задержки.",
                    limits.target_per_second, limits.max_per_second
                ),
                Lang::En => format!(
                    "Peak is {sec} events per second: above the safe mark of {}, though under the limit of {}. Some players may hear lag.",
                    limits.target_per_second, limits.max_per_second
                ),
            },
        ),
        Status::Over => (
            tr("Не пройдёт в SS14", "Too dense for SS14").to_string(),
            match crate::i18n::lang() {
                Lang::Ru => format!(
                    "Перегруженных мест: {overloads}. Пик — {sec} событий в секунду (лимит {}) и {frame} за кадр (лимит {}). В игре инструмент начнёт отставать, а потом остановится. Обработайте файл в режиме «Безопасный».",
                    limits.max_per_second, limits.frame_limit
                ),
                Lang::En => format!(
                    "Overloaded spots: {overloads}. Peak is {sec} events per second (limit {}) and {frame} per frame (limit {}). In game the instrument will lag and then stop. Process the file in “Safe” mode.",
                    limits.max_per_second, limits.frame_limit
                ),
            },
        ),
    }
}

fn level(v: u32, target: u32, max: u32) -> Color32 {
    if v > max {
        theme::OVER
    } else if v > target {
        theme::WARN
    } else {
        theme::OK
    }
}

/// Показатель: подпись, число, примечание и полоска заполнения.
fn gauge(ui: &mut Ui, caption: &str, value: String, note: String, bar: Option<(f32, Color32)>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(132.0, 56.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::RAISED);
    p.text(rect.left_top() + Vec2::new(10.0, 6.0), Align2::LEFT_TOP, caption, ui_font(12.0), theme::TEXT_DIM);
    p.text(rect.left_top() + Vec2::new(10.0, 22.0), Align2::LEFT_TOP, value, mono(16.0), theme::TEXT);
    if !note.is_empty() {
        p.text(rect.right_top() + Vec2::new(-8.0, 26.0), Align2::RIGHT_TOP, note, ui_font(11.5), theme::TEXT_FAINT);
    }
    if let Some((frac, color)) = bar {
        let track = Rect::from_min_size(Pos2::new(rect.left() + 10.0, rect.bottom() - 9.0), Vec2::new(rect.width() - 20.0, 3.0));
        p.rect_filled(track, 1.0, theme::LINE);
        p.rect_filled(Rect::from_min_size(track.min, Vec2::new(track.width() * frac.clamp(0.0, 1.0), 3.0)), 1.0, color);
    }
}

/// Полоса перемотки на всю оставшуюся ширину. Возвращает выбранное время, секунды.
fn seek_bar(ui: &mut Ui, pos_s: f64, total_s: f64, enabled: bool) -> Option<f64> {
    let w = ui.available_width().max(80.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width() - 12.0, 4.0));
    let frac = if total_s > 0.0 { (pos_s / total_s).clamp(0.0, 1.0) as f32 } else { 0.0 };
    p.rect_filled(track, 2.0, theme::LINE_STRONG);
    p.rect_filled(
        Rect::from_min_size(track.min, Vec2::new(track.width() * frac, track.height())),
        2.0,
        if enabled { theme::ACCENT } else { theme::TEXT_FAINT },
    );
    if enabled {
        let r = if resp.hovered() || resp.dragged() { 7.0 } else { 5.5 };
        p.circle_filled(Pos2::new(track.left() + track.width() * frac, track.center().y), r, theme::TEXT);
    }
    let to_s = |x: f32| ((x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * total_s;
    if let Some(hover) = resp.hover_pos() {
        resp.clone().on_hover_text_at_pointer(fmt_time(to_s(hover.x)));
    }
    if resp.clicked() || resp.dragged() {
        return resp.interact_pointer_pos().map(|p| to_s(p.x));
    }
    None
}

fn short_path(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max { s.to_string() } else { format!("…{}", s.chars().skip(n - max + 1).collect::<String>()) }
}

fn truncate_to(p: &egui::Painter, text: &str, font: egui::FontId, max_w: f32) -> String {
    let fits = |s: String| p.layout_no_wrap(s, font.clone(), Color32::WHITE).size().x <= max_w;
    if fits(text.to_string()) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(chars[..mid].iter().collect::<String>() + "…") {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(c);
    }
    out
}
