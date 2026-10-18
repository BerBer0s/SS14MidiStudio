//! Операции редактора над песней. Нота адресуется тройкой (канал, начало, клавиша):
//! после нормализации на одной клавише канала не бывает двух нот с одним началом.

use std::collections::HashSet;

use ss14midi_core::build::normalize_song;
use ss14midi_core::model::{Control, ControlKind};
use ss14midi_core::{Note, Song};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoteId {
    pub ch: u8,
    pub start: u64,
    pub key: u8,
}

impl NoteId {
    pub fn of(ch: usize, n: &Note) -> NoteId {
        NoteId { ch: ch as u8, start: n.start, key: n.key }
    }
}

pub type Selection = HashSet<NoteId>;

pub fn find(song: &Song, id: NoteId) -> Option<usize> {
    let notes = &song.channels.get(id.ch as usize)?.notes;
    let from = notes.partition_point(|n| n.start < id.start);
    notes[from..].iter().take_while(|n| n.start == id.start).position(|n| n.key == id.key).map(|i| from + i)
}

pub fn get(song: &Song, id: NoteId) -> Option<Note> {
    find(song, id).map(|i| song.channels[id.ch as usize].notes[i])
}

/// Вынуть выбранные ноты из песни (возвращаются с каналом).
fn take(song: &mut Song, sel: &Selection) -> Vec<(u8, Note)> {
    let mut out = Vec::new();
    for (ch, c) in song.channels.iter_mut().enumerate() {
        c.notes.retain(|n| {
            if sel.contains(&NoteId::of(ch, n)) {
                out.push((ch as u8, *n));
                false
            } else {
                true
            }
        });
    }
    out
}

fn put_back(song: &mut Song, notes: Vec<(u8, Note)>) -> Selection {
    let mut sel = Selection::new();
    for (ch, n) in notes {
        sel.insert(NoteId::of(ch as usize, &n));
        song.channels[ch as usize].notes.push(n);
    }
    normalize_song(song);
    // После нормализации часть нот могла склеиться — оставляем только существующие.
    sel.retain(|id| find(song, *id).is_some());
    sel
}

/// Сдвиг выбранных нот на `dt` тиков и `dk` полутонов.
pub fn move_notes(song: &mut Song, sel: &Selection, dt: i64, dk: i32) -> Selection {
    let notes = take(song, sel)
        .into_iter()
        .map(|(ch, n)| {
            let start = (n.start as i64 + dt).max(0) as u64;
            let shift = start as i64 - n.start as i64;
            let key = (n.key as i32 + dk).clamp(0, 127) as u8;
            (ch, Note { start, end: (n.end as i64 + shift) as u64, key, velocity: n.velocity })
        })
        .collect();
    put_back(song, notes)
}

/// Изменение длины выбранных нот на `dt` тиков (не короче `min_len`).
pub fn resize_notes(song: &mut Song, sel: &Selection, dt: i64, min_len: u64) -> Selection {
    let notes = take(song, sel)
        .into_iter()
        .map(|(ch, n)| {
            let end = (n.end as i64 + dt).max((n.start + min_len.max(1)) as i64) as u64;
            (ch, Note { end, ..n })
        })
        .collect();
    put_back(song, notes)
}

pub fn delete_notes(song: &mut Song, sel: &Selection) -> usize {
    take(song, sel).len()
}

pub fn add_note(song: &mut Song, ch: u8, note: Note) -> NoteId {
    song.channels[ch as usize].notes.push(note);
    normalize_song(song);
    NoteId::of(ch as usize, &note)
}

/// Сдвиг громкости выбранных нот.
pub fn shift_velocity(song: &mut Song, sel: &Selection, dv: i32) {
    for (ch, c) in song.channels.iter_mut().enumerate() {
        for n in &mut c.notes {
            if sel.contains(&NoteId::of(ch, n)) {
                n.velocity = (n.velocity as i32 + dv).clamp(1, 127) as u8;
            }
        }
    }
}

/// Квантование начал выбранных нот к сетке `grid` тиков.
pub fn quantize(song: &mut Song, sel: &Selection, grid: u64) -> Selection {
    let grid = grid.max(1);
    let notes = take(song, sel)
        .into_iter()
        .map(|(ch, n)| {
            let start = ((n.start + grid / 2) / grid) * grid;
            let len = n.len();
            (ch, Note { start, end: start + len, ..n })
        })
        .collect();
    put_back(song, notes)
}

/// Копия выбранных нот относительно самой ранней.
pub fn copy(song: &Song, sel: &Selection) -> Vec<(u8, Note)> {
    let mut out: Vec<(u8, Note)> = Vec::new();
    for (ch, c) in song.channels.iter().enumerate() {
        out.extend(c.notes.iter().filter(|n| sel.contains(&NoteId::of(ch, n))).map(|n| (ch as u8, *n)));
    }
    let base = out.iter().map(|(_, n)| n.start).min().unwrap_or(0);
    for (_, n) in &mut out {
        n.start -= base;
        n.end -= base;
    }
    out
}

pub fn paste(song: &mut Song, clip: &[(u8, Note)], at: u64) -> Selection {
    let notes = clip
        .iter()
        .map(|(ch, n)| (*ch, Note { start: n.start + at, end: n.end + at, ..*n }))
        .collect();
    put_back(song, notes)
}

/// Выделить всё в видимых каналах.
pub fn select_all(song: &Song, hidden: u16) -> Selection {
    let mut sel = Selection::new();
    for (ch, c) in song.channels.iter().enumerate() {
        if hidden & (1 << ch) == 0 {
            sel.extend(c.notes.iter().map(|n| NoteId::of(ch, n)));
        }
    }
    sel
}

/// Сменить инструмент канала: program change на тике 0 (остальные смены — как были).
pub fn set_program(song: &mut Song, ch: usize, program: u8) {
    let c = &mut song.channels[ch];
    c.controls.retain(|x| !(x.kind == ControlKind::Program && x.tick == 0));
    c.controls.insert(0, Control { tick: 0, kind: ControlKind::Program, value: program as u16 });
    let first_note = c.notes.first().map(|n| n.start).unwrap_or(0);
    // Если первый program change стоял до первой ноты, но не на нуле — он перебил бы выбор.
    c.controls.retain(|x| !(x.kind == ControlKind::Program && x.tick > 0 && x.tick <= first_note));
}

pub fn clear_channel(song: &mut Song, ch: usize) {
    song.channels[ch].notes.clear();
    song.channels[ch].controls.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ss14midi_core::Division;

    fn song() -> Song {
        let mut s = Song::empty(Division::Ticks(96));
        s.channels[0].notes = vec![
            Note { start: 0, end: 96, key: 60, velocity: 100 },
            Note { start: 96, end: 192, key: 62, velocity: 90 },
        ];
        s.channels[1].notes = vec![Note { start: 0, end: 48, key: 40, velocity: 80 }];
        s
    }

    fn id(ch: u8, start: u64, key: u8) -> NoteId {
        NoteId { ch, start, key }
    }

    #[test]
    fn move_and_reselect() {
        let mut s = song();
        let sel: Selection = [id(0, 0, 60)].into();
        let new = move_notes(&mut s, &sel, 48, 2);
        assert_eq!(new, [id(0, 48, 62)].into());
        // Переехавшая нота наложилась на 62@96 — первая обрезана до её начала.
        assert_eq!(get(&s, id(0, 48, 62)).unwrap().end, 96);
    }

    #[test]
    fn move_clamps_at_zero() {
        let mut s = song();
        let new = move_notes(&mut s, &[id(1, 0, 40)].into(), -500, 0);
        assert_eq!(new, [id(1, 0, 40)].into());
    }

    #[test]
    fn resize_respects_min_length() {
        let mut s = song();
        resize_notes(&mut s, &[id(0, 96, 62)].into(), -1000, 6);
        assert_eq!(get(&s, id(0, 96, 62)).unwrap().end, 102);
    }

    #[test]
    fn delete_add_velocity() {
        let mut s = song();
        assert_eq!(delete_notes(&mut s, &[id(0, 0, 60), id(1, 0, 40)].into()), 2);
        let n = add_note(&mut s, 3, Note { start: 10, end: 20, key: 70, velocity: 50 });
        shift_velocity(&mut s, &[n].into(), 100);
        assert_eq!(get(&s, n).unwrap().velocity, 127);
        assert_eq!(s.note_count(), 2);
    }

    #[test]
    fn copy_paste_quantize() {
        let mut s = song();
        let clip = copy(&s, &[id(0, 96, 62), id(1, 0, 40)].into());
        let pasted = paste(&mut s, &clip, 1000);
        assert!(pasted.contains(&id(0, 1096, 62)) && pasted.contains(&id(1, 1000, 40)));
        let q = quantize(&mut s, &[id(0, 1096, 62)].into(), 48);
        assert_eq!(q, [id(0, 1104, 62)].into());
    }

    #[test]
    fn program_change_replaces_initial() {
        let mut s = song();
        s.channels[0].controls.push(Control { tick: 0, kind: ControlKind::Program, value: 3 });
        set_program(&mut s, 0, 40);
        assert_eq!(s.channels[0].first_program(), Some(40));
        assert_eq!(s.channels[0].controls.iter().filter(|c| c.kind == ControlKind::Program).count(), 1);
    }
}
