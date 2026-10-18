//! Главное окно: состояние приложения, файлы, правки, воспроизведение и горячие клавиши.
//! Отрисовка панелей — в `panels.rs`, окна настроек и справки — в `dialogs.rs`.

mod dialogs;
mod panels;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use egui::{Color32, Key, KeyboardShortcut, Modifiers, Ui};
use serde::{Deserialize, Serialize};
use ss14midi_core::batch::{collect_inputs, is_midi, is_our_output};
use ss14midi_core::{Settings, Song};

use crate::editor::{self, Selection};
use crate::i18n::{count, set_lang, tr, Lang};
use crate::library::{output_path, Analysis, Entry, EntryState, History, Jobs, Msg, Source};
use crate::player::{build_events, PlayEvent, Player};
use crate::theme;
use crate::views::roll::{RollAction, RollView, Tool};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub lang: Lang,
    pub settings: Settings,
    pub out_dir: Option<PathBuf>,
    pub last_dir: Option<PathBuf>,
    pub midi_port: Option<String>,
    pub simulate: bool,
    pub follow: bool,
    pub show_ghosts: bool,
    pub snap_div: u32,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            lang: Lang::Ru,
            settings: Settings::default(),
            out_dir: None,
            last_dir: None,
            midi_port: None,
            simulate: false,
            follow: true,
            show_ghosts: true,
            snap_div: 4,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SideTab {
    Channels,
    Report,
}

struct Toast {
    text: String,
    color: Color32,
    at: f64,
}

pub struct StudioApp {
    entries: Vec<Entry>,
    selected: Option<u64>,
    next_id: u64,
    rx: Receiver<Msg>,
    jobs: Jobs,
    prefs: Prefs,
    player: Player,
    roll: RollView,
    mute: u16,
    solo: u16,
    sent_mute: u16,
    ab_original: bool,
    cursor_us: u64,
    was_playing: bool,
    side_tab: SideTab,
    show_settings: bool,
    show_help: bool,
    toast: Option<Toast>,
    analyzing: HashMap<u64, u64>,
    save_after: HashSet<u64>,
    /// События для проигрывателя по ключу (файл, версия, исходник ли).
    events_cache: Option<(EventsKey, Arc<Vec<PlayEvent>>)>,
    title: String,
}

type EventsKey = (u64, u64, bool);

const PREFS_KEY: &str = "ss14-midi-studio-prefs";

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> StudioApp {
        theme::install(&cc.egui_ctx);
        let prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, PREFS_KEY)).unwrap_or_default();
        set_lang(prefs.lang);
        let (tx, rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        let repaint_ctx = cc.egui_ctx.clone();
        let player = Player::new(prefs.midi_port.clone(), move || repaint_ctx.request_repaint());
        player.set_simulate(prefs.simulate);
        let mut roll = RollView::default();
        roll.snap_div = prefs.snap_div;
        roll.show_ghosts = prefs.show_ghosts;
        let mut app = StudioApp {
            entries: Vec::new(),
            selected: None,
            next_id: 1,
            rx,
            jobs: Jobs { tx, ctx },
            prefs,
            player,
            roll,
            mute: 0,
            solo: 0,
            sent_mute: 0,
            ab_original: false,
            cursor_us: 0,
            was_playing: false,
            side_tab: SideTab::Channels,
            show_settings: false,
            show_help: false,
            toast: None,
            analyzing: HashMap::new(),
            save_after: HashSet::new(),
            events_cache: None,
            title: String::new(),
        };
        // Файлы, переданные при запуске (например, «Открыть с помощью»).
        let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
        if !args.is_empty() {
            app.add_paths(args);
        }
        app
    }

    fn toast(&mut self, ctx: &egui::Context, text: impl Into<String>, color: Color32) {
        self.toast = Some(Toast { text: text.into(), color, at: ctx.input(|i| i.time) });
    }

    fn entry(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn entry_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.selected.and_then(|id| self.entry(id))
    }

    fn select(&mut self, id: u64) {
        if self.selected == Some(id) {
            return;
        }
        self.selected = Some(id);
        self.roll.needs_fit = true;
        self.roll.selection.clear();
        self.roll.hidden = 0;
        self.mute = 0;
        self.solo = 0;
        self.cursor_us = 0;
        self.player.stop();
        if let Some(e) = self.entry(id) {
            self.roll.active_channel = first_used_channel(&e.working);
        }
    }

    // ── Файлы ───────────────────────────────────────────────────────────────

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                dirs.push(p);
            } else if is_midi(&p) {
                files.push((p, None));
            }
        }
        let (found, _) = collect_inputs(&dirs);
        files.extend(found.into_iter().map(|f| (f.path, f.root)));
        let mut first_new = None;
        for (path, root) in files {
            if self.entries.iter().any(|e| e.path == path) {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            if let Some(parent) = path.parent() {
                self.prefs.last_dir = Some(root.clone().unwrap_or_else(|| parent.to_path_buf()));
            }
            self.entries.push(Entry {
                id,
                path: path.clone(),
                root,
                state: EntryState::Loading,
                original: None,
                source: None,
                working: Song::empty(ss14midi_core::Division::Ticks(480)),
                history: History::default(),
                revision: 0,
                analysis: None,
                analyzed_revision: u64::MAX,
                report: None,
                ghosts: Vec::new(),
                busy: false,
                dirty: false,
                saved_to: None,
            });
            self.jobs.load(id, path, self.prefs.settings.limits);
            first_new.get_or_insert(id);
        }
        if let (None, Some(id)) = (self.selected, first_new) {
            self.select(id);
        }
    }

    fn pick_files(&mut self) {
        let mut d = rfd::FileDialog::new().add_filter("MIDI", &["mid", "midi", "rmi"]);
        if let Some(dir) = &self.prefs.last_dir {
            d = d.set_directory(dir);
        }
        if let Some(files) = d.pick_files() {
            self.add_paths(files);
        }
    }

    fn pick_folder(&mut self) {
        let mut d = rfd::FileDialog::new();
        if let Some(dir) = &self.prefs.last_dir {
            d = d.set_directory(dir);
        }
        if let Some(dir) = d.pick_folder() {
            self.add_paths(vec![dir]);
        }
    }

    fn pick_out_dir(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.prefs.out_dir = Some(dir);
        }
    }

    fn remove_entry(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
        if self.selected == Some(id) {
            self.player.stop();
            self.selected = None;
            if let Some(first) = self.entries.first().map(|e| e.id) {
                self.select(first);
            }
        }
    }

    fn close_all(&mut self) {
        self.player.stop();
        self.entries.clear();
        self.selected = None;
    }

    fn save_target(&self, e: &Entry) -> PathBuf {
        if is_our_output(&e.path) {
            return e.path.clone();
        }
        output_path(e, self.prefs.out_dir.as_deref())
    }

    // ── Фоновые задачи ──────────────────────────────────────────────────────

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded { id, result } => {
                    let selected = self.selected == Some(id);
                    let Some(e) = self.entry_mut(id) else { continue };
                    match result {
                        Ok(loaded) => {
                            let (loaded, analysis) = *loaded;
                            e.original = Some(Arc::new(loaded.song.clone()));
                            e.source = Some(Source { analysis, issues: loaded.build, problems: loaded.song.source.problems.clone() });
                            e.working = loaded.song;
                            e.state = EntryState::Ready;
                            if selected {
                                let first = first_used_channel(&e.working);
                                self.roll.needs_fit = true;
                                self.roll.active_channel = first;
                            }
                        }
                        Err(err) => e.state = EntryState::Failed(err),
                    }
                }
                Msg::Analyzed { id, revision, analysis } => {
                    self.analyzing.remove(&id);
                    if let Some(e) = self.entry_mut(id)
                        && revision == e.revision
                    {
                        e.analysis = Some(analysis);
                        e.analyzed_revision = revision;
                    }
                }
                Msg::Processed { id, revision, song, report } => {
                    let save = self.save_after.remove(&id);
                    let Some(e) = self.entry_mut(id) else { continue };
                    e.busy = false;
                    if e.revision != revision {
                        let name = e.name();
                        self.toast(ctx, format!("{name}: {}", tr("файл изменился во время обработки, результат отброшен", "file changed while processing, result discarded")), theme::WARN);
                        continue;
                    }
                    let before = std::mem::replace(&mut e.working, song);
                    e.commit(tr("Обработка", "Processing"), before);
                    e.ghosts = report.changes.removed_notes.clone();
                    e.analysis = Some(Analysis {
                        status: report.status,
                        metrics: report.after.clone(),
                        timeline: report.timeline_after.clone(),
                        ranges: report.overloaded_ranges.clone(),
                    });
                    e.analyzed_revision = e.revision;
                    e.report = Some(*report);
                    if save {
                        let e = self.entry(id).unwrap();
                        self.jobs.save(id, e.working.clone(), self.save_target(e));
                    }
                    if self.selected == Some(id) {
                        self.roll.selection.clear();
                    }
                }
                Msg::Saved { id, path, result } => {
                    let Some(e) = self.entry_mut(id) else { continue };
                    match result {
                        Ok(()) => {
                            e.dirty = false;
                            e.saved_to = Some(path.clone());
                            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            self.toast(ctx, format!("{} {name}", tr("Сохранено:", "Saved:")), theme::OK);
                        }
                        Err(err) => self.toast(ctx, format!("{} {err}", tr("Не удалось сохранить:", "Save failed:")), theme::OVER),
                    }
                }
            }
        }
        // Анализ догоняет правки.
        let limits = self.prefs.settings.limits;
        let mut todo = Vec::new();
        for e in &self.entries {
            if e.is_ready() && e.analysis_stale() && !e.busy && self.analyzing.get(&e.id) != Some(&e.revision) {
                todo.push((e.id, e.revision, e.working.clone()));
            }
        }
        for (id, rev, song) in todo {
            self.analyzing.insert(id, rev);
            self.jobs.analyze(id, rev, song, limits);
        }
    }

    fn process(&mut self, id: u64, save: bool) {
        let settings = self.prefs.settings.clone();
        let Some(e) = self.entry_mut(id) else { return };
        if !e.is_ready() || e.busy {
            return;
        }
        e.busy = true;
        let (rev, song) = (e.revision, e.working.clone());
        if save {
            self.save_after.insert(id);
        }
        self.jobs.process(id, rev, song, settings);
    }

    fn process_all(&mut self) {
        let ids: Vec<u64> = self.entries.iter().filter(|e| e.is_ready() && !e.busy).map(|e| e.id).collect();
        for id in ids {
            self.process(id, true);
        }
    }

    fn save(&mut self, id: u64, path: Option<PathBuf>) {
        let Some(e) = self.entry(id).filter(|e| e.is_ready()) else { return };
        let target = path.unwrap_or_else(|| self.save_target(e));
        self.jobs.save(id, e.working.clone(), target);
    }

    fn save_as(&mut self, id: u64) {
        let Some(e) = self.entry(id) else { return };
        let target = self.save_target(e);
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut d = rfd::FileDialog::new().add_filter("MIDI", &["mid"]).set_file_name(name);
        if let Some(dir) = target.parent() {
            d = d.set_directory(dir);
        }
        if let Some(path) = d.save_file() {
            self.save(id, Some(path));
        }
    }

    // ── Правки ──────────────────────────────────────────────────────────────

    fn edit(&mut self, label: &str, f: impl FnOnce(&mut Song, &mut Selection)) {
        let Some(id) = self.selected else { return };
        let mut sel = std::mem::take(&mut self.roll.selection);
        let Some(e) = self.entry_mut(id) else { return };
        if !e.is_ready() || e.busy {
            self.roll.selection = sel;
            return;
        }
        let before = e.working.clone();
        f(&mut e.working, &mut sel);
        if e.working != before {
            e.commit(label, before);
        }
        self.roll.selection = sel;
    }

    fn apply_roll(&mut self, actions: Vec<RollAction>) {
        for a in actions {
            match a {
                RollAction::Move { dt, dk } => self.edit(tr("Перемещение", "Move"), |s, sel| *sel = editor::move_notes(s, sel, dt, dk)),
                RollAction::Resize { dt } => {
                    let min = self.selected_entry().map(|e| self.roll.grid(&e.working)).unwrap_or(1).min(30);
                    self.edit(tr("Длина", "Length"), |s, sel| *sel = editor::resize_notes(s, sel, dt, min));
                }
                RollAction::Delete(ids) => self.edit(tr("Удаление", "Delete"), |s, sel| {
                    editor::delete_notes(s, &ids);
                    sel.retain(|x| !ids.contains(x));
                }),
                RollAction::Add { ch, note } => self.edit(tr("Новая нота", "New note"), |s, sel| *sel = [editor::add_note(s, ch, note)].into()),
                RollAction::Velocity { ids, dv } => self.edit(tr("Громкость", "Velocity"), |s, _| editor::shift_velocity(s, &ids, dv)),
                RollAction::Restore(i) => self.restore_ghosts(&[i]),
                RollAction::Seek(tick) => {
                    if let Some(e) = self.selected_entry() {
                        let us = e.working.tempo_map().tick_to_us(tick);
                        self.seek(us);
                    }
                }
            }
        }
    }

    fn restore_ghosts(&mut self, which: &[usize]) {
        let Some(id) = self.selected else { return };
        let Some(e) = self.entry_mut(id) else { return };
        let notes: Vec<_> = which
            .iter()
            .filter_map(|&i| e.ghosts.get(i))
            .map(|g| (g.channel - 1, ss14midi_core::Note { start: g.start, end: g.end, key: g.key, velocity: g.velocity }))
            .collect();
        let mut sorted = which.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for i in sorted {
            if i < e.ghosts.len() {
                e.ghosts.remove(i);
            }
        }
        self.edit(tr("Возврат нот", "Restore notes"), |s, sel| {
            sel.clear();
            for (ch, n) in notes {
                sel.insert(editor::add_note(s, ch, n));
            }
        });
    }

    fn undo(&mut self, redo: bool) {
        let Some(id) = self.selected else { return };
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else { return };
        let label = if redo { e.history.redo(&mut e.working) } else { e.history.undo(&mut e.working) };
        if label.is_some() {
            e.touch();
            let working = &e.working;
            self.roll.selection.retain(|x| editor::find(working, *x).is_some());
        }
    }

    fn reset_to_original(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(e) = self.entry_mut(id) else { return };
        let Some(orig) = e.original.clone() else { return };
        let before = std::mem::replace(&mut e.working, (*orig).clone());
        e.commit(tr("Сброс к исходнику", "Reset to source"), before);
        e.ghosts.clear();
        e.report = None;
        self.roll.selection.clear();
    }

    // ── Воспроизведение ─────────────────────────────────────────────────────

    fn play_song(&self) -> Option<(u64, u64, Arc<Song>)> {
        let e = self.selected_entry().filter(|e| e.is_ready())?;
        if self.ab_original {
            e.original.clone().map(|o| (e.id, u64::MAX, o))
        } else {
            Some((e.id, e.revision, Arc::new(e.working.clone())))
        }
    }

    fn toggle_play(&mut self) {
        if self.player.is_playing() {
            self.cursor_us = self.player.position_us();
            self.player.stop();
        } else {
            self.start_playback(self.cursor_us);
        }
    }

    fn stop(&mut self) {
        self.player.stop();
        self.cursor_us = 0;
    }

    fn start_playback(&mut self, from_us: u64) {
        let Some((id, rev, song)) = self.play_song() else { return };
        let key = (id, rev, self.ab_original);
        let events = match &self.events_cache {
            Some((k, ev)) if *k == key => ev.clone(),
            _ => {
                let ev = Arc::new(build_events(&song));
                self.events_cache = Some((key, ev.clone()));
                ev
            }
        };
        self.player.play(events, from_us);
    }

    fn seek(&mut self, us: u64) {
        self.cursor_us = us;
        if self.player.is_playing() {
            self.start_playback(us);
        }
    }

    fn set_ab(&mut self, original: bool) {
        if self.ab_original == original {
            return;
        }
        self.ab_original = original;
        if self.player.is_playing() {
            let pos = self.player.position_us();
            self.start_playback(pos);
        }
    }

    fn position_us(&self) -> u64 {
        if self.player.is_playing() { self.player.position_us() } else { self.cursor_us }
    }

    fn duration_s(&self) -> f64 {
        self.selected_entry().and_then(|e| e.analysis.as_ref()).map(|a| a.metrics.duration_s).unwrap_or(0.0)
    }

    // ── Клавиатура ──────────────────────────────────────────────────────────

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let cmd = Modifiers::COMMAND;
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if pressed(cmd | Modifiers::SHIFT, Key::Z) || pressed(cmd, Key::Y) {
            self.undo(true);
        }
        if pressed(cmd, Key::Z) {
            self.undo(false);
        }
        if pressed(cmd | Modifiers::SHIFT, Key::S)
            && let Some(id) = self.selected
        {
            self.save_as(id);
        }
        if pressed(cmd, Key::S)
            && let Some(id) = self.selected
        {
            self.save(id, None);
        }
        if pressed(cmd, Key::O) {
            self.pick_files();
        }
        if pressed(cmd, Key::Enter)
            && let Some(id) = self.selected
        {
            self.process(id, false);
        }
        if pressed(cmd, Key::A)
            && let Some(e) = self.selected_entry()
        {
            self.roll.selection = editor::select_all(&e.working, self.roll.hidden);
        }
        if pressed(cmd, Key::C)
            && let Some(e) = self.selected_entry()
        {
            self.roll.clipboard = editor::copy(&e.working, &self.roll.selection);
        }
        if pressed(cmd, Key::X)
            && let Some(e) = self.selected_entry()
        {
            self.roll.clipboard = editor::copy(&e.working, &self.roll.selection);
            let ids = self.roll.selection.clone();
            self.edit(tr("Вырезать", "Cut"), |s, sel| {
                editor::delete_notes(s, &ids);
                sel.clear();
            });
        }
        if pressed(cmd, Key::V) && !self.roll.clipboard.is_empty() {
            let at = self
                .selected_entry()
                .map(|e| e.working.tempo_map().us_to_tick(self.position_us() as f64))
                .unwrap_or(0)
                .max(self.roll.scroll_tick as u64);
            let clip = self.roll.clipboard.clone();
            self.edit(tr("Вставка", "Paste"), |s, sel| *sel = editor::paste(s, &clip, at));
        }
        if pressed(Modifiers::NONE, Key::Delete) || pressed(Modifiers::NONE, Key::Backspace) {
            let ids = self.roll.selection.clone();
            if !ids.is_empty() {
                self.edit(tr("Удаление", "Delete"), |s, sel| {
                    editor::delete_notes(s, &ids);
                    sel.clear();
                });
            }
        }
        if pressed(Modifiers::NONE, Key::Escape) {
            self.roll.selection.clear();
            self.show_settings = false;
            self.show_help = false;
        }
        let grid = self.selected_entry().map(|e| self.roll.grid(&e.working) as i64).unwrap_or(1);
        for (key, dt, dk) in [(Key::ArrowUp, 0, 1), (Key::ArrowDown, 0, -1), (Key::ArrowLeft, -1, 0), (Key::ArrowRight, 1, 0)] {
            if pressed(Modifiers::SHIFT, key) && !self.roll.selection.is_empty() {
                self.edit(tr("Сдвиг", "Nudge"), |s, sel| *sel = editor::move_notes(s, sel, dt * grid * 4, dk * 12));
            } else if pressed(Modifiers::NONE, key) && !self.roll.selection.is_empty() {
                self.edit(tr("Сдвиг", "Nudge"), |s, sel| *sel = editor::move_notes(s, sel, dt * grid, dk));
            }
        }
        if pressed(Modifiers::NONE, Key::Q) && !self.roll.selection.is_empty() {
            let g = grid as u64;
            self.edit(tr("Квантование", "Quantize"), |s, sel| *sel = editor::quantize(s, sel, g));
        }
        if pressed(Modifiers::NONE, Key::Space) {
            self.toggle_play();
        }
        if pressed(Modifiers::NONE, Key::Home) {
            self.seek(0);
        }
        if pressed(Modifiers::NONE, Key::V) {
            self.roll.tool = Tool::Select;
        }
        if pressed(Modifiers::NONE, Key::D) || pressed(Modifiers::NONE, Key::B) {
            self.roll.tool = Tool::Draw;
        }
        if pressed(Modifiers::NONE, Key::F) {
            self.roll.needs_fit = true;
        }
        if pressed(Modifiers::NONE, Key::Plus) || pressed(Modifiers::NONE, Key::Equals) {
            self.roll.zoom_x(1.25, self.roll.view_width / 2.0);
        }
        if pressed(Modifiers::NONE, Key::Minus) {
            self.roll.zoom_x(0.8, self.roll.view_width / 2.0);
        }
        if pressed(Modifiers::NONE, Key::F1) {
            self.show_help = !self.show_help;
        }
    }

    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if dropped.is_empty() {
            return;
        }
        let before = self.entries.len();
        self.add_paths(dropped);
        let added = self.entries.len() - before;
        if added == 0 {
            self.toast(ctx, tr("Новых MIDI-файлов не найдено", "No new MIDI files found"), theme::WARN);
        } else {
            self.toast(ctx, format!("{} {}", tr("Добавлено:", "Added:"), count(added, ["файл", "файла", "файлов"], ["file", "files"])), theme::OK);
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.selected_entry() {
            Some(e) => format!("{}{} — SS14 MIDI Studio", e.name(), if e.dirty { " •" } else { "" }),
            None => "SS14 MIDI Studio".to_string(),
        };
        if title != self.title {
            self.title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_messages(&ctx);
        self.shortcuts(&ctx);
        self.handle_drop(&ctx);

        // Mute/solo → проигрыватель.
        let mask = if self.solo != 0 { !self.solo } else { self.mute };
        if mask != self.sent_mute {
            self.sent_mute = mask;
            self.player.set_mute(mask);
        }
        let playing = self.player.is_playing();
        if self.was_playing && !playing && self.player.position_us() as f64 / 1e6 >= self.duration_s() - 0.05 {
            // Доиграло до конца.
            self.cursor_us = 0;
        }
        self.was_playing = playing;
        if playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        if self.entries.iter().any(|e| e.busy || matches!(e.state, EntryState::Loading)) || !self.analyzing.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }

        self.layout(ui);
        if self.show_settings {
            self.settings_window(&ctx);
        }
        if self.show_help {
            self.help_window(&ctx);
        }
        self.overlays(&ctx);
        self.update_title(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFS_KEY, &self.prefs);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.player.stop();
    }
}

fn first_used_channel(song: &Song) -> u8 {
    (0..16).find(|&c| !song.channels[c].notes.is_empty()).unwrap_or(0) as u8
}

fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(not(windows))]
    {
        if let Some(dir) = path.parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}
