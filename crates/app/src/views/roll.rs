//! Пианоролл: просмотр и редактирование нот.

use egui::{Align2, Color32, CursorIcon, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use serde::{Deserialize, Serialize};
use ss14midi_core::gm::note_name;
use ss14midi_core::model::{MetaKind, DRUM_CHANNEL};
use ss14midi_core::passes::limiter::RemovedNote;
use ss14midi_core::timing::TempoMap;
use ss14midi_core::{Note, Song};

use crate::editor::{NoteId, Selection};
use crate::i18n::tr;
use crate::theme::{self, mono};
use crate::widgets;

pub const KEYS_W: f32 = 46.0;
pub const RULER_H: f32 = 22.0;
pub const VEL_H: f32 = 62.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tool {
    Select,
    Draw,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    Move { anchor: NoteId, dt: i64, dk: i32 },
    Resize { anchor: NoteId, dt: i64 },
    Box { from: Pos2, to: Pos2, additive: bool },
    Draw { key: u8, start: u64, end: u64 },
    Velocity { dv: i32, origin_y: f32, targets_all: bool, note: NoteId },
    Pan,
}

pub enum RollAction {
    Move { dt: i64, dk: i32 },
    Resize { dt: i64 },
    Delete(Selection),
    Add { ch: u8, note: Note },
    Velocity { ids: Selection, dv: i32 },
    Restore(usize),
    Seek(u64),
}

pub struct RollView {
    pub px_per_tick: f32,
    pub key_h: f32,
    pub scroll_tick: f64,
    pub top_key: f32,
    pub tool: Tool,
    /// Делений на четверть для привязки (0 — без привязки).
    pub snap_div: u32,
    pub selection: Selection,
    pub hidden: u16,
    pub active_channel: u8,
    pub show_ghosts: bool,
    pub clipboard: Vec<(u8, Note)>,
    pub needs_fit: bool,
    pub hover: Option<(usize, Note)>,
    pub last_velocity: u8,
    /// Ширина области нот в последнем кадре — для навигации.
    pub view_width: f32,
    drag: Option<Drag>,
}

impl Default for RollView {
    fn default() -> RollView {
        RollView {
            px_per_tick: 0.1,
            key_h: 10.0,
            scroll_tick: 0.0,
            top_key: 96.0,
            tool: Tool::Select,
            snap_div: 4,
            selection: Selection::new(),
            hidden: 0,
            active_channel: 0,
            show_ghosts: true,
            clipboard: Vec::new(),
            needs_fit: true,
            hover: None,
            last_velocity: 100,
            view_width: 800.0,
            drag: None,
        }
    }
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

pub fn quarter(song: &Song) -> u64 {
    song.division.quarter_ticks()
}

impl RollView {
    pub fn grid(&self, song: &Song) -> u64 {
        if self.snap_div == 0 { 1 } else { (quarter(song) / self.snap_div as u64).max(1) }
    }

    fn snap_round(&self, song: &Song, tick: i64, free: bool) -> i64 {
        let g = if free { 1 } else { self.grid(song) as i64 };
        ((tick as f64 / g as f64).round() as i64) * g
    }

    fn snap_floor(&self, song: &Song, tick: i64, free: bool) -> i64 {
        let g = if free { 1 } else { self.grid(song) as i64 };
        tick.div_euclid(g) * g
    }

    /// Видимый диапазон тиков.
    pub fn visible_ticks(&self) -> (f64, f64) {
        (self.scroll_tick, self.scroll_tick + (self.view_width / self.px_per_tick) as f64)
    }

    /// Показать песню целиком по высоте и первые ~20 секунд по ширине.
    pub fn fit(&mut self, song: &Song, width: f32, height: f32) {
        let map = song.tempo_map();
        let end = song.end_tick().max(quarter(song) * 4);
        let twenty = map.us_to_tick(20_000_000.0).max(quarter(song) * 8);
        let span = end.min(twenty).max(1);
        self.px_per_tick = (width.max(100.0) / span as f32).clamp(0.0005, 8.0);
        self.scroll_tick = 0.0;
        let keys = song.channels.iter().flat_map(|c| c.notes.iter().map(|n| n.key));
        let (lo, hi) = keys.fold((127u8, 0u8), |(lo, hi), k| (lo.min(k), hi.max(k)));
        let (lo, hi) = if lo > hi { (48, 84) } else { (lo, hi) };
        let rows = (hi - lo) as f32 + 7.0;
        self.key_h = (height / rows).clamp(5.0, 16.0);
        let visible_rows = height / self.key_h;
        self.top_key = (hi as f32 + 3.0).max(lo as f32 + visible_rows - 3.0).min(128.0);
        self.needs_fit = false;
    }

    pub fn center_on(&mut self, tick: f64) {
        let (a, b) = self.visible_ticks();
        self.scroll_tick = (tick - (b - a) / 2.0).max(0.0);
    }

    /// Держать позицию воспроизведения в кадре.
    pub fn follow(&mut self, tick: f64) {
        let (a, b) = self.visible_ticks();
        if tick < a || tick > a + (b - a) * 0.85 {
            self.scroll_tick = (tick - (b - a) * 0.1).max(0.0);
        }
    }

    pub fn zoom_x(&mut self, factor: f32, anchor_x: f32) {
        let before = self.scroll_tick + (anchor_x / self.px_per_tick) as f64;
        self.px_per_tick = (self.px_per_tick * factor).clamp(0.0005, 8.0);
        self.scroll_tick = (before - (anchor_x / self.px_per_tick) as f64).max(0.0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        song: &Song,
        map: &TempoMap,
        ghosts: &[RemovedNote],
        overloads_ticks: &[(u64, u64)],
        playhead_tick: Option<u64>,
    ) -> Vec<RollAction> {
        let mut actions = Vec::new();
        let full = ui.available_rect_before_wrap();
        let notes_rect = Rect::from_min_max(
            Pos2::new(full.left() + KEYS_W, full.top() + RULER_H),
            Pos2::new(full.right(), full.bottom() - VEL_H - 1.0),
        );
        let ruler_rect = Rect::from_min_max(Pos2::new(notes_rect.left(), full.top()), Pos2::new(full.right(), notes_rect.top()));
        let keys_rect = Rect::from_min_max(Pos2::new(full.left(), notes_rect.top()), Pos2::new(notes_rect.left(), notes_rect.bottom()));
        let vel_rect = Rect::from_min_max(Pos2::new(notes_rect.left(), notes_rect.bottom() + 1.0), Pos2::new(full.right(), full.bottom()));
        ui.allocate_rect(full, Sense::hover());
        if self.needs_fit && notes_rect.width() > 50.0 {
            self.fit(song, notes_rect.width(), notes_rect.height());
        }
        self.view_width = notes_rect.width();

        let notes_resp = ui.interact(notes_rect, ui.id().with("roll-notes"), Sense::click_and_drag());
        let ruler_resp = ui.interact(ruler_rect, ui.id().with("roll-ruler"), Sense::click_and_drag());
        let vel_resp = ui.interact(vel_rect, ui.id().with("roll-vel"), Sense::click_and_drag());
        let (mods, scroll, zoom) = ui.input(|i| (i.modifiers, i.smooth_scroll_delta, i.zoom_delta()));
        let free = mods.alt;

        // Прокрутка и масштаб колесом.
        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()).filter(|p| full.contains(*p)) {
            if zoom != 1.0 {
                if mods.alt {
                    let key_at = self.top_key - (pos.y - notes_rect.top()) / self.key_h;
                    self.key_h = (self.key_h * zoom).clamp(4.0, 28.0);
                    self.top_key = key_at + (pos.y - notes_rect.top()) / self.key_h;
                } else {
                    self.zoom_x(zoom, pos.x - notes_rect.left());
                }
            }
            if scroll != Vec2::ZERO {
                if mods.shift || scroll.x.abs() > scroll.y.abs() {
                    let dx = if scroll.x != 0.0 { scroll.x } else { scroll.y };
                    self.scroll_tick = (self.scroll_tick - (dx / self.px_per_tick) as f64).max(0.0);
                } else {
                    self.top_key += scroll.y / self.key_h;
                }
            }
        }
        let rows_visible = notes_rect.height() / self.key_h;
        self.top_key = self.top_key.clamp(rows_visible.min(128.0), 128.0);

        let (scroll_tick, px, top_key, key_h) = (self.scroll_tick, self.px_per_tick, self.top_key, self.key_h);
        let x_of = move |t: f64| notes_rect.left() + ((t - scroll_tick) as f32) * px;
        let tick_at = move |x: f32| scroll_tick + ((x - notes_rect.left()) / px) as f64;
        let y_of = move |key: u8| notes_rect.top() + (top_key - key as f32 - 1.0) * key_h;
        let key_at = move |y: f32| (top_key - (y - notes_rect.top()) / key_h).floor().clamp(0.0, 127.0) as u8;
        let (t0, t1) = (scroll_tick, scroll_tick + (notes_rect.width() / px) as f64);

        let painter = ui.painter_at(full);
        // Фон: строки клавиш.
        painter.rect_filled(notes_rect, 0.0, theme::PANEL);
        let first_key = key_at(notes_rect.bottom());
        let last_key = key_at(notes_rect.top());
        for key in first_key..=last_key {
            let y = y_of(key);
            let row = Rect::from_min_size(Pos2::new(notes_rect.left(), y), Vec2::new(notes_rect.width(), self.key_h));
            if is_black(key) {
                painter.rect_filled(row, 0.0, Color32::from_rgb(0x10, 0x12, 0x14));
            }
            if key % 12 == 0 {
                painter.hline(notes_rect.x_range(), y + self.key_h, Stroke::new(1.0, theme::LINE));
            }
        }

        // Сетка: такты, доли, деления привязки.
        let q = quarter(song) as f64;
        let (num, den) = song
            .meta
            .iter()
            .find_map(|m| match m.kind {
                MetaKind::TimeSignature(ts) if m.tick == 0 => Some((ts[0].max(1) as f64, 2f64.powi(ts[1] as i32))),
                _ => None,
            })
            .unwrap_or((4.0, 4.0));
        let beat = q * 4.0 / den;
        let bar = beat * num;
        let mut bar_every = 1.0;
        while bar * bar_every * self.px_per_tick as f64 > 0.0 && bar * bar_every * (self.px_per_tick as f64) < 36.0 {
            bar_every *= 2.0;
        }
        let sub = if self.snap_div == 0 { beat } else { q / self.snap_div as f64 };
        let draw_lines = |step: f64, stroke: Stroke| {
            if step * self.px_per_tick as f64 >= 6.0 {
                let mut t = (t0 / step).floor() * step;
                while t <= t1 {
                    painter.vline(x_of(t).round() + 0.5, notes_rect.y_range(), stroke);
                    t += step;
                }
            }
        };
        draw_lines(sub, Stroke::new(1.0, Color32::from_rgb(0x1A, 0x1D, 0x21)));
        draw_lines(beat, Stroke::new(1.0, Color32::from_rgb(0x22, 0x26, 0x2B)));
        draw_lines(bar * bar_every, Stroke::new(1.0, theme::LINE_STRONG));

        // Перегруженные места.
        for &(a, b) in overloads_ticks {
            if (b as f64) < t0 || (a as f64) > t1 {
                continue;
            }
            let r = Rect::from_x_y_ranges(x_of(a as f64)..=x_of(b as f64).max(x_of(a as f64) + 2.0), notes_rect.y_range());
            painter.rect_filled(r.intersect(notes_rect), 0.0, theme::OVER.gamma_multiply(0.07));
        }

        // «Призраки» удалённых ограничителем нот.
        let mut ghost_hit: Option<usize> = None;
        let pointer = notes_resp.hover_pos();
        if self.show_ghosts {
            for (i, g) in ghosts.iter().enumerate() {
                if (g.end as f64) < t0 || (g.start as f64) > t1 || self.hidden & (1 << (g.channel - 1)) != 0 {
                    continue;
                }
                let r = Rect::from_min_max(
                    Pos2::new(x_of(g.start as f64), y_of(g.key)),
                    Pos2::new(x_of(g.end as f64).max(x_of(g.start as f64) + 2.0), y_of(g.key) + self.key_h),
                );
                painter.rect_stroke(r.shrink(0.5), 0.0, Stroke::new(1.0, theme::OVER.gamma_multiply(0.75)), StrokeKind::Inside);
                if pointer.is_some_and(|p| r.contains(p)) {
                    ghost_hit = Some(i);
                }
            }
        }

        // Ноты.
        let dragging_sel = matches!(self.drag, Some(Drag::Move { .. } | Drag::Resize { .. }));
        let mut visible: Vec<(Rect, NoteId, Note)> = Vec::new();
        for (ch, c) in song.channels.iter().enumerate() {
            if self.hidden & (1 << ch) != 0 || c.notes.is_empty() {
                continue;
            }
            let color = theme::channel_color(ch);
            let max_len = c.notes.iter().map(|n| n.len()).max().unwrap_or(0) as f64;
            let from = c.notes.partition_point(|n| (n.start as f64) < t0 - max_len);
            for n in &c.notes[from..] {
                if n.start as f64 > t1 {
                    break;
                }
                if (n.end as f64) < t0 || n.key < first_key || n.key > last_key {
                    continue;
                }
                let id = NoteId::of(ch, n);
                let x0 = x_of(n.start as f64);
                let x1 = x_of(n.end as f64).max(x0 + 1.5);
                let r = Rect::from_min_max(Pos2::new(x0, y_of(n.key) + 0.5), Pos2::new(x1, y_of(n.key) + self.key_h - 0.5));
                let selected = self.selection.contains(&id);
                let strength = 0.35 + 0.65 * (n.velocity as f32 / 127.0);
                let fill = if selected && dragging_sel { color.gamma_multiply(0.25) } else { color.gamma_multiply(strength) };
                painter.rect_filled(r, 0.0, fill);
                if selected {
                    painter.rect_stroke(r, 0.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
                } else if r.width() > 4.0 {
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_black_alpha(90)), StrokeKind::Inside);
                }
                visible.push((r, id, *n));
            }
        }

        let hit = |p: Pos2| visible.iter().rev().find(|(r, _, _)| r.expand2(Vec2::new(1.0, 0.0)).contains(p)).copied();
        self.hover = pointer.and_then(&hit).map(|(_, id, n)| (id.ch as usize, n));

        // Взаимодействие с областью нот.
        let press = ui.input(|i| i.pointer.press_origin());
        if notes_resp.drag_started_by(PointerButton::Middle) {
            self.drag = Some(Drag::Pan);
        } else if notes_resp.drag_started_by(PointerButton::Primary)
            && let Some(origin) = press
        {
            if let Some((r, id, n)) = hit(origin) {
                if !self.selection.contains(&id) {
                    if !mods.shift && !mods.ctrl {
                        self.selection.clear();
                    }
                    self.selection.insert(id);
                }
                let edge = (r.right() - origin.x) <= 6.0_f32.min(r.width() * 0.35);
                self.drag = Some(if edge { Drag::Resize { anchor: id, dt: 0 } } else { Drag::Move { anchor: id, dt: 0, dk: 0 } });
                self.last_velocity = n.velocity;
            } else if self.tool == Tool::Draw {
                let start = self.snap_floor(song, tick_at(origin.x) as i64, free).max(0) as u64;
                self.drag = Some(Drag::Draw { key: key_at(origin.y), start, end: start + self.grid(song) });
            } else {
                self.drag = Some(Drag::Box { from: origin, to: origin, additive: mods.shift || mods.ctrl });
            }
        }
        if let (Some(drag), Some(pos)) = (self.drag, ui.input(|i| i.pointer.interact_pos())) {
            let origin = press.unwrap_or(pos);
            match drag {
                Drag::Pan => {
                    let d = ui.input(|i| i.pointer.delta());
                    self.scroll_tick = (self.scroll_tick - (d.x / self.px_per_tick) as f64).max(0.0);
                    self.top_key += d.y / self.key_h;
                }
                Drag::Move { anchor, .. } => {
                    let a = crate::editor::get(song, anchor).map(|n| n.start as i64).unwrap_or(0);
                    let raw = a + ((pos.x - origin.x) / self.px_per_tick).round() as i64;
                    let dt = self.snap_round(song, raw, free).max(0) - a;
                    let dk = ((origin.y - pos.y) / self.key_h).round() as i32;
                    self.drag = Some(Drag::Move { anchor, dt, dk });
                }
                Drag::Resize { anchor, .. } => {
                    let e = crate::editor::get(song, anchor).map(|n| n.end as i64).unwrap_or(0);
                    let raw = e + ((pos.x - origin.x) / self.px_per_tick).round() as i64;
                    self.drag = Some(Drag::Resize { anchor, dt: self.snap_round(song, raw, free) - e });
                }
                Drag::Box { from, additive, .. } => self.drag = Some(Drag::Box { from, to: pos, additive }),
                Drag::Draw { key, start, .. } => {
                    let end = (self.snap_round(song, tick_at(pos.x) as i64, free).max(0) as u64).max(start + self.grid(song));
                    self.drag = Some(Drag::Draw { key, start, end });
                }
                Drag::Velocity { origin_y, targets_all, note, .. } => {
                    let dv = ((origin_y - pos.y) / vel_rect.height() * 127.0).round() as i32;
                    self.drag = Some(Drag::Velocity { dv, origin_y, targets_all, note });
                }
            }
        }
        if self.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
            actions.extend(self.finish_drag(&visible));
        }

        if notes_resp.clicked()
            && let Some(p) = notes_resp.interact_pointer_pos()
        {
            match hit(p) {
                Some((_, id, _)) => {
                    if mods.shift || mods.ctrl {
                        if !self.selection.remove(&id) {
                            self.selection.insert(id);
                        }
                    } else {
                        self.selection = [id].into();
                    }
                }
                None if self.tool == Tool::Draw => {
                    let start = self.snap_floor(song, tick_at(p.x) as i64, free).max(0) as u64;
                    let note = Note { start, end: start + self.grid(song).max(quarter(song) / 4), key: key_at(p.y), velocity: self.last_velocity };
                    actions.push(RollAction::Add { ch: self.active_channel, note });
                }
                None => {
                    if !mods.shift {
                        self.selection.clear();
                    }
                }
            }
        }
        if notes_resp.double_clicked() && self.tool == Tool::Select
            && let Some(p) = notes_resp.interact_pointer_pos()
        {
            if let Some(g) = ghost_hit.filter(|_| hit(p).is_none()) {
                actions.push(RollAction::Restore(g));
            } else if hit(p).is_none() {
                let start = self.snap_floor(song, tick_at(p.x) as i64, free).max(0) as u64;
                let note = Note { start, end: start + self.grid(song).max(quarter(song) / 4), key: key_at(p.y), velocity: self.last_velocity };
                actions.push(RollAction::Add { ch: self.active_channel, note });
            }
        }
        if notes_resp.secondary_clicked()
            && let Some((_, id, _)) = notes_resp.interact_pointer_pos().and_then(&hit)
        {
            let del = if self.selection.contains(&id) { self.selection.clone() } else { [id].into() };
            actions.push(RollAction::Delete(del));
        }
        // Курсор.
        if let Some(p) = pointer {
            let icon = match (self.drag, hit(p)) {
                (Some(Drag::Pan), _) => CursorIcon::Grabbing,
                (Some(Drag::Move { .. }), _) => CursorIcon::Grabbing,
                (Some(Drag::Resize { .. }), _) => CursorIcon::ResizeHorizontal,
                (None, Some((r, _, _))) if (r.right() - p.x) <= 6.0_f32.min(r.width() * 0.35) => CursorIcon::ResizeHorizontal,
                (None, Some(_)) => CursorIcon::Grab,
                (None, None) if self.tool == Tool::Draw => CursorIcon::Crosshair,
                _ => CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        // Предпросмотр перетаскивания.
        let preview = match self.drag {
            Some(Drag::Move { dt, dk, .. }) => Some((dt, dk, false)),
            Some(Drag::Resize { dt, .. }) => Some((dt, 0, true)),
            _ => None,
        };
        if let Some((dt, dk, resize)) = preview.filter(|_| !self.selection.is_empty()) {
                for (_, id, n) in &visible {
                    if !self.selection.contains(id) {
                        continue;
                    }
                    let (s, e, k) = if resize {
                        (n.start as i64, (n.end as i64 + dt).max(n.start as i64 + 1), n.key as i32)
                    } else {
                        ((n.start as i64 + dt).max(0), (n.end as i64 + dt).max(1), (n.key as i32 + dk).clamp(0, 127))
                    };
                    let r = Rect::from_min_max(
                        Pos2::new(x_of(s as f64), y_of(k as u8) + 0.5),
                        Pos2::new(x_of(e as f64).max(x_of(s as f64) + 1.5), y_of(k as u8) + self.key_h - 0.5),
                    );
                    painter.rect_filled(r, 0.0, theme::channel_color(id.ch as usize));
                    painter.rect_stroke(r, 0.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
                }
        }
        match self.drag {
            Some(Drag::Box { from, to, .. }) => {
                let r = Rect::from_two_pos(from, to);
                painter.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.08));
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
            }
            Some(Drag::Draw { key, start, end }) => {
                let r = Rect::from_min_max(Pos2::new(x_of(start as f64), y_of(key)), Pos2::new(x_of(end as f64), y_of(key) + self.key_h));
                painter.rect_filled(r, 0.0, theme::channel_color(self.active_channel as usize));
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
            }
            _ => {}
        }

        // Клавиатура слева.
        painter.rect_filled(keys_rect, 0.0, theme::RAISED);
        for key in first_key..=last_key {
            let y = y_of(key);
            let r = Rect::from_min_size(Pos2::new(keys_rect.left(), y), Vec2::new(KEYS_W, self.key_h));
            if is_black(key) {
                painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(KEYS_W * 0.62, self.key_h)), 0.0, theme::INK);
            } else {
                painter.rect_filled(r.shrink2(Vec2::new(0.0, 0.5)), 0.0, Color32::from_rgb(0xC9, 0xC5, 0xBB));
            }
            if key % 12 == 0 && self.key_h >= 7.0 {
                painter.text(Pos2::new(keys_rect.right() - 3.0, y + self.key_h / 2.0), Align2::RIGHT_CENTER, note_name(key), mono(9.0), theme::INK);
            }
            if self.hover.is_some_and(|(_, n)| n.key == key) {
                painter.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.5));
            }
        }
        painter.vline(keys_rect.right(), keys_rect.y_range(), Stroke::new(1.0, theme::LINE_STRONG));

        // Линейка.
        painter.rect_filled(ruler_rect, 0.0, theme::RAISED);
        for &(a, b) in overloads_ticks {
            let x0 = x_of(a as f64).max(ruler_rect.left());
            let x1 = x_of(b as f64).min(ruler_rect.right());
            if x1 > x0 {
                painter.rect_filled(Rect::from_x_y_ranges(x0..=x1.max(x0 + 2.0), (ruler_rect.bottom() - 3.0)..=ruler_rect.bottom()), 0.0, theme::OVER);
            }
        }
        let mut t = (t0 / (bar * bar_every)).floor() * bar * bar_every;
        while t <= t1 {
            let x = x_of(t);
            if x >= ruler_rect.left() - 1.0 {
                painter.vline(x, (ruler_rect.top() + 8.0)..=ruler_rect.bottom(), Stroke::new(1.0, theme::TEXT_FAINT));
                let bar_no = (t / bar).round() as u64 + 1;
                painter.text(Pos2::new(x + 3.0, ruler_rect.top() + 3.0), Align2::LEFT_TOP, bar_no.to_string(), mono(10.0), theme::TEXT_DIM);
            }
            t += bar * bar_every;
        }
        painter.hline(ruler_rect.x_range(), ruler_rect.bottom(), Stroke::new(1.0, theme::LINE_STRONG));
        let corner = Rect::from_min_max(full.min, Pos2::new(keys_rect.right(), ruler_rect.bottom()));
        painter.rect_filled(corner, 0.0, theme::RAISED);
        let label = if map.bpm_at(0, song.division) > 0.0 { format!("{:.0}", map.bpm_at(t0.max(0.0) as u64, song.division)) } else { String::new() };
        painter.text(corner.center(), Align2::CENTER_CENTER, format!("{label}♩"), mono(10.0), theme::TEXT_DIM);
        if let Some(pos) = ruler_resp.interact_pointer_pos()
            && (ruler_resp.clicked() || ruler_resp.dragged())
        {
            actions.push(RollAction::Seek(tick_at(pos.x).max(0.0) as u64));
        }
        ruler_resp.on_hover_cursor(CursorIcon::PointingHand);

        // Полоса громкости.
        painter.rect_filled(vel_rect, 0.0, theme::RAISED);
        painter.hline(vel_rect.x_range(), vel_rect.top() - 0.5, Stroke::new(1.0, theme::LINE_STRONG));
        painter.text(Pos2::new(full.left() + 4.0, vel_rect.center().y), Align2::LEFT_CENTER, tr("громк.", "vel."), mono(10.0), theme::TEXT_FAINT);
        let vel_drag = match self.drag {
            Some(Drag::Velocity { dv, targets_all, note, .. }) => Some((dv, targets_all, note)),
            _ => None,
        };
        let mut nearest: Option<(f32, NoteId, Note)> = None;
        let vp = vel_resp.hover_pos().or(press.filter(|p| vel_rect.contains(*p)));
        for (r, id, n) in &visible {
            let x = r.left();
            let mut v = n.velocity as i32;
            if let Some((dv, all, note)) = vel_drag
                && ((all && self.selection.contains(id)) || *id == note)
            {
                v = (v + dv).clamp(1, 127);
            }
            let h = (vel_rect.height() - 6.0) * v as f32 / 127.0;
            let selected = self.selection.contains(id);
            let color = if selected { theme::ACCENT } else { theme::channel_color(id.ch as usize).gamma_multiply(0.8) };
            painter.vline(x + 0.5, (vel_rect.bottom() - h)..=vel_rect.bottom(), Stroke::new(if selected { 2.0 } else { 1.0 }, color));
            painter.rect_filled(Rect::from_center_size(Pos2::new(x + 0.5, vel_rect.bottom() - h), Vec2::splat(3.0)), 0.0, color);
            if let Some(p) = vp {
                let d = (p.x - x).abs();
                if d <= 5.0 && nearest.is_none_or(|(bd, _, _)| d < bd) {
                    nearest = Some((d, *id, *n));
                }
            }
        }
        if vel_resp.drag_started_by(PointerButton::Primary)
            && let (Some((_, id, _)), Some(p)) = (nearest, press)
        {
            let all = self.selection.contains(&id);
            self.drag = Some(Drag::Velocity { dv: 0, origin_y: p.y, targets_all: all, note: id });
        }
        if nearest.is_some() {
            vel_resp.on_hover_cursor(CursorIcon::ResizeVertical);
        }

        // Позиция воспроизведения.
        if let Some(p) = playhead_tick {
            let x = x_of(p as f64);
            if x >= notes_rect.left() && x <= notes_rect.right() {
                painter.vline(x, full.y_range(), Stroke::new(1.5, theme::ACCENT));
                painter.add(egui::Shape::convex_polygon(
                    vec![Pos2::new(x - 5.0, ruler_rect.top()), Pos2::new(x + 5.0, ruler_rect.top()), Pos2::new(x, ruler_rect.top() + 7.0)],
                    theme::ACCENT,
                    Stroke::NONE,
                ));
            }
        }
        // Метка канала для рисования.
        if self.tool == Tool::Draw {
            let ch = self.active_channel as usize;
            let drums = if ch == DRUM_CHANNEL { tr(" (ударные)", " (drums)") } else { "" };
            let txt = format!("{} {}{drums}", tr("Рисование в канал", "Drawing into channel"), ch + 1);
            widgets::pill(&painter, Pos2::new(notes_rect.right() - 6.0, notes_rect.top() + 6.0), Align2::RIGHT_TOP, &txt, theme::channel_color(ch));
        }
        actions
    }

    fn finish_drag(&mut self, visible: &[(Rect, NoteId, Note)]) -> Vec<RollAction> {
        let drag = self.drag.take();
        let mut out = Vec::new();
        match drag {
            Some(Drag::Move { dt, dk, .. }) if dt != 0 || dk != 0 => out.push(RollAction::Move { dt, dk }),
            Some(Drag::Resize { dt, .. }) if dt != 0 => out.push(RollAction::Resize { dt }),
            Some(Drag::Box { from, to, additive }) => {
                let r = Rect::from_two_pos(from, to);
                if !additive {
                    self.selection.clear();
                }
                for (nr, id, _) in visible {
                    if nr.intersects(r) {
                        self.selection.insert(*id);
                    }
                }
            }
            Some(Drag::Draw { key, start, end }) => out.push(RollAction::Add {
                ch: self.active_channel,
                note: Note { start, end, key, velocity: self.last_velocity },
            }),
            Some(Drag::Velocity { dv, targets_all, note, .. }) if dv != 0 => {
                let ids = if targets_all { self.selection.clone() } else { [note].into() };
                out.push(RollAction::Velocity { ids, dv });
            }
            _ => {}
        }
        out
    }
}
