//! Окна настроек и справки.

use egui::{Align2, RichText, Ui, Vec2};
use ss14midi_core::Settings;
use ss14midi_core::passes::velocity::VelocityMode;

use super::StudioApp;
use crate::i18n::tr;
use crate::theme::{self, bold, mono};
use crate::widgets;

fn section(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).font(bold(14.0)).color(theme::TEXT));
    ui.add_space(2.0);
}

fn note(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).small().color(theme::TEXT_FAINT)).wrap());
}

impl StudioApp {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new(RichText::new(tr("Настройки", "Settings")).font(bold(16.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .anchor(Align2::RIGHT_TOP, Vec2::new(-12.0, 56.0))
            .show(ctx, |ui| {
                section(ui, tr("Прослушивание", "Playback"));
                let port = self.player.shared.port.lock().unwrap().clone();
                let ports = self.player.shared.ports.lock().unwrap().clone();
                let err = self.player.shared.error.lock().unwrap().clone();
                ui.horizontal(|ui| {
                    ui.label(tr("MIDI-выход", "MIDI output"));
                    let resp = egui::ComboBox::from_id_salt("midi-port")
                        .width(260.0)
                        .selected_text(if port.is_empty() { tr("не выбран", "none").to_string() } else { port.clone() })
                        .show_ui(ui, |ui| {
                            for p in &ports {
                                if ui.selectable_label(*p == port, p).clicked() {
                                    self.player.set_port(p.clone());
                                    self.prefs.midi_port = Some(p.clone());
                                }
                            }
                        });
                    if resp.response.clicked() {
                        self.player.refresh_ports();
                    }
                });
                if let Some(err) = err {
                    ui.label(RichText::new(err).small().color(theme::WARN));
                }
                note(ui, tr("На Windows — Microsoft GS Wavetable Synth: тот же набор звуков, что у инструментов SS14.", "On Windows use Microsoft GS Wavetable Synth: the same sound set SS14 instruments use."));

                section(ui, tr("Сохранение", "Saving"));
                ui.horizontal_wrapped(|ui| {
                    let where_ = match &self.prefs.out_dir {
                        Some(d) => d.display().to_string(),
                        None => tr("рядом с исходными файлами", "next to the source files").to_string(),
                    };
                    ui.label(format!("{} {where_}", tr("Папка:", "Folder:")));
                    if widgets::button(ui, tr("Изменить…", "Change…")).clicked() {
                        self.pick_out_dir();
                    }
                    if self.prefs.out_dir.is_some() && widgets::button(ui, tr("Рядом с исходными", "Next to sources")).clicked() {
                        self.prefs.out_dir = None;
                    }
                });
                note(ui, tr("К имени результата добавляется «_ss14».", "“_ss14” is added to the result's name."));

                let s = &mut self.prefs.settings;
                section(ui, tr("Громкость при обработке", "Volume when processing"));
                egui::Grid::new("vel").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("Предел для ударных", "Drum ceiling"));
                    ui.add(egui::Slider::new(&mut s.velocity.drum_cap, 1..=127));
                    ui.end_row();
                    ui.label(tr("Предел для мелодии", "Melodic ceiling"));
                    ui.add(egui::Slider::new(&mut s.velocity.melodic_cap, 1..=127));
                    ui.end_row();
                    ui.label(tr("Выравнивать", "Level"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut s.velocity.mode, VelocityMode::Song, tr("всю песню вместе", "whole song together"));
                        ui.radio_value(&mut s.velocity.mode, VelocityMode::PerChannel, tr("каждый канал", "each channel"));
                    });
                    ui.end_row();
                    ui.label(tr("Приглушать аккорды", "Soften chords"));
                    ui.checkbox(&mut s.velocity.chord_softening, "");
                    ui.end_row();
                });

                section(ui, tr("Лимиты SS14", "SS14 limits"));
                egui::Grid::new("lim").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("Цель, событий в секунду", "Target, events per second"));
                    ui.add(egui::DragValue::new(&mut s.limits.target_per_second).range(100..=s.limits.max_per_second));
                    ui.end_row();
                    ui.label(tr("Лимит сервера, в секунду", "Server limit, per second"));
                    ui.add(egui::DragValue::new(&mut s.limits.max_per_second).range(100..=10000));
                    ui.end_row();
                    ui.label(tr("Лимит за кадр", "Per-frame limit"));
                    ui.add(egui::DragValue::new(&mut s.limits.frame_limit).range(10..=1000));
                    ui.end_row();
                    ui.label(tr("Микроноты короче, мс", "Micro notes shorter than, ms"));
                    ui.add(egui::Slider::new(&mut s.limiter.micro_note_ms, 0.0..=100.0));
                    ui.end_row();
                    ui.label(tr("Прореживание кривых, мс", "Curve thinning, ms"));
                    ui.add(egui::Slider::new(&mut s.cleanup.cc_min_interval_ms, 1.0..=100.0));
                    ui.end_row();
                });
                note(ui, tr(
                    "По умолчанию 900 / 1000 / 60: лимиты midi.max_events_per_second и midi.max_events_per_batch на серверах SS14, с запасом. Изменения применяются при следующей обработке.",
                    "Defaults 900 / 1000 / 60 follow midi.max_events_per_second and midi.max_events_per_batch on SS14 servers, with headroom. Changes apply on the next processing run.",
                ));
                ui.add_space(8.0);
                if widgets::button(ui, tr("Сбросить по умолчанию", "Reset to defaults")).clicked() {
                    self.prefs.settings = Settings::new(self.prefs.settings.preset);
                }
            });
        self.show_settings = open;
    }

    pub(super) fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new(RichText::new(tr("Справка", "Help")).font(bold(16.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                section(ui, tr("Как пользоваться", "How to use"));
                for line in [
                    tr(
                        "1. Перетащите MIDI-файлы или папку в окно. Рядом с каждым файлом появится оценка: пройдёт ли он лимиты SS14.",
                        "1. Drop MIDI files or a folder onto the window. Each file gets a verdict: whether it fits SS14 limits.",
                    ),
                    tr(
                        "2. Выберите режим обработки и нажмите «Обработать». Обработку можно отменить, как любую правку.",
                        "2. Pick a processing mode and press “Process”. Processing can be undone like any edit.",
                    ),
                    tr(
                        "3. Послушайте результат, сравните с исходником, при желании поправьте ноты в пианоролле.",
                        "3. Listen to the result, compare it with the source, tweak notes in the piano roll if you like.",
                    ),
                    tr(
                        "4. Нажмите «Сохранить». Для многих файлов есть «Обработать и сохранить все» слева.",
                        "4. Press “Save”. For many files use “Process and save all” on the left.",
                    ),
                ] {
                    ui.add(egui::Label::new(line).wrap());
                }
                section(ui, tr("Клавиши", "Keys"));
                let rows = [
                    ("Space", tr("Слушать / пауза", "Play / pause")),
                    ("Home", tr("В начало", "To start")),
                    ("V / D", tr("Выделение / рисование", "Select / draw")),
                    ("Ctrl+Z / Ctrl+Y", tr("Отменить / повторить", "Undo / redo")),
                    ("Ctrl+A", tr("Выделить всё", "Select all")),
                    ("Ctrl+C / X / V", tr("Копировать / вырезать / вставить", "Copy / cut / paste")),
                    ("Del", tr("Удалить выделенное", "Delete selection")),
                    ("↑ ↓", tr("Сдвиг на полутон (Shift — на октаву)", "Transpose by a semitone (Shift: octave)")),
                    ("← →", tr("Сдвиг по сетке (Shift — ×4)", "Nudge by grid (Shift: ×4)")),
                    ("Q", tr("Выровнять по сетке", "Quantize to grid")),
                    (tr("Ctrl + колесо", "Ctrl + wheel"), tr("Масштаб по времени", "Zoom time")),
                    (tr("Ctrl + Alt + колесо", "Ctrl + Alt + wheel"), tr("Масштаб по высоте", "Zoom pitch")),
                    (tr("Shift + колесо", "Shift + wheel"), tr("Прокрутка по времени", "Scroll time")),
                    (tr("Средняя кнопка", "Middle button"), tr("Двигать вид", "Pan view")),
                    (tr("Правый клик по ноте", "Right-click a note"), tr("Удалить ноту", "Delete note")),
                    (tr("Alt при перетаскивании", "Alt while dragging"), tr("Без привязки к сетке", "No grid snap")),
                    (tr("Двойной клик", "Double-click"), tr("Новая нота / вернуть убранную", "New note / restore removed")),
                    ("Ctrl+Enter", tr("Обработать файл", "Process file")),
                    ("Ctrl+S / Ctrl+Shift+S", tr("Сохранить / сохранить как", "Save / save as")),
                    ("Ctrl+O", tr("Открыть файлы", "Open files")),
                    ("F", tr("Показать с начала", "Fit from the start")),
                ];
                egui::Grid::new("keys").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    for (k, what) in rows {
                        ui.label(RichText::new(k).font(mono(12.5)).color(theme::ACCENT));
                        ui.label(what);
                        ui.end_row();
                    }
                });
            });
        self.show_help = open;
    }
}
