//! Сырые дорожки → модель песни. Здесь же происходит починка «по построению»:
//! пустые note_off исчезают, наложения разбираются, зависшие ноты закрываются.

use std::collections::VecDeque;

use serde::Serialize;

use crate::model::{Control, MetaEvent, Note, SourceInfo, Song, SysEx, CHANNELS};
use crate::read::{RawKind, RawSong};

pub const MAX_CHANNEL_NAME: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BuildStats {
    pub orphan_offs: u32,
    pub overlaps_resolved: u32,
    pub same_start_merged: u32,
    pub hanging_closed: u32,
    pub zero_length_extended: u32,
}

pub fn build_song(raw: &RawSong) -> (Song, BuildStats) {
    let mut stats = BuildStats::default();
    let mut song = Song::empty(raw.division);
    song.source = SourceInfo {
        format: raw.format,
        tracks: raw.tracks.len(),
        recovered: raw.recovered(),
        problems: raw.problems.clone(),
    };
    if raw.format == 2 {
        song.source.problems.push("формат 2 (независимые последовательности) — обрабатываю как формат 1".into());
    }

    // Устойчивое слияние канальных событий всех дорожек: (тик, дорожка, порядок в дорожке).
    let mut order: Vec<(u64, u32, u32)> = Vec::new();
    for (ti, track) in raw.tracks.iter().enumerate() {
        for (ei, e) in track.events.iter().enumerate() {
            if matches!(
                e.kind,
                RawKind::NoteOn { .. } | RawKind::NoteOff { .. } | RawKind::Control { .. } | RawKind::SysEx(_)
            ) {
                order.push((e.tick, ti as u32, ei as u32));
            }
        }
    }
    order.sort_unstable();

    let mut open: Vec<VecDeque<(u64, u8)>> = vec![VecDeque::new(); CHANNELS * 128];
    let mut uses_track = vec![vec![false; raw.tracks.len()]; CHANNELS];
    let mut notes: Vec<Vec<Note>> = vec![Vec::new(); CHANNELS];

    for &(tick, ti, ei) in &order {
        match &raw.tracks[ti as usize].events[ei as usize].kind {
            RawKind::NoteOn { ch, key, vel } => {
                uses_track[*ch as usize][ti as usize] = true;
                open[*ch as usize * 128 + *key as usize].push_back((tick, *vel));
            }
            RawKind::NoteOff { ch, key } => match open[*ch as usize * 128 + *key as usize].pop_front() {
                Some((start, velocity)) => {
                    push_note(&mut notes[*ch as usize], &mut stats, start, tick, *key, velocity);
                }
                None => stats.orphan_offs += 1,
            },
            RawKind::Control { ch, kind, value } => {
                uses_track[*ch as usize][ti as usize] = true;
                song.channels[*ch as usize].controls.push(Control { tick, kind: *kind, value: *value });
            }
            RawKind::SysEx(data) => song.sysex.push(SysEx { tick, data: data.clone() }),
            _ => {}
        }
    }

    // Зависшие ноты закрываются на конце песни.
    let song_end = raw.end_tick();
    for (slot, queue) in open.iter_mut().enumerate() {
        let (ch, key) = (slot / 128, (slot % 128) as u8);
        for (start, velocity) in queue.drain(..) {
            let end = if song_end > start { song_end } else { start + raw.division.quarter_ticks() };
            stats.hanging_closed += 1;
            push_note(&mut notes[ch], &mut stats, start, end, key, velocity);
        }
    }

    for (ch, list) in notes.into_iter().enumerate() {
        song.channels[ch].notes = resolve_overlaps(list, &mut stats);
    }

    song.meta = collect_meta(raw);

    let track_names: Vec<Option<String>> = raw
        .tracks
        .iter()
        .map(|t| {
            t.events.iter().find_map(|e| match &e.kind {
                RawKind::TrackName(bytes) => decode_name(bytes),
                _ => None,
            })
        })
        .collect();
    for (channel, used_by) in song.channels.iter_mut().zip(&uses_track) {
        let mut names: Vec<&str> = Vec::new();
        for (used, name) in used_by.iter().zip(&track_names) {
            if let (true, Some(name)) = (*used, name)
                && !names.contains(&name.as_str())
            {
                names.push(name);
            }
        }
        if !names.is_empty() {
            channel.name = Some(truncate_chars(&names.join(", "), MAX_CHANNEL_NAME));
        }
    }

    (song, stats)
}

fn push_note(list: &mut Vec<Note>, stats: &mut BuildStats, start: u64, mut end: u64, key: u8, velocity: u8) {
    if end <= start {
        end = start + 1;
        stats.zero_length_extended += 1;
    }
    list.push(Note { start, end, key, velocity });
}

/// На одной клавише ноты не должны перекрываться: синтезатор отпускает все голоса клавиши
/// по первому note_off, поэтому перекрытие звучит как обрыв.
fn resolve_overlaps(mut list: Vec<Note>, stats: &mut BuildStats) -> Vec<Note> {
    list.sort_unstable_by_key(|n| (n.key, n.start, n.end));
    let mut out: Vec<Note> = Vec::with_capacity(list.len());
    for n in list {
        if let Some(prev) = out.last_mut()
            && prev.key == n.key
            && n.start < prev.end
        {
            if n.start == prev.start {
                prev.end = prev.end.max(n.end);
                prev.velocity = prev.velocity.max(n.velocity);
                stats.same_start_merged += 1;
                continue;
            }
            prev.end = n.start;
            stats.overlaps_resolved += 1;
        }
        out.push(n);
    }
    out.sort_unstable_by_key(|n| (n.start, n.key));
    out
}

/// Темп, размер и тональность со всех дорожек. На одном тике одного типа побеждает дорожка
/// с меньшим номером, внутри неё — последнее событие.
fn collect_meta(raw: &RawSong) -> Vec<MetaEvent> {
    let mut all = Vec::new();
    for (ti, track) in raw.tracks.iter().enumerate() {
        for (ei, e) in track.events.iter().enumerate() {
            if let RawKind::Meta(kind) = e.kind {
                all.push((e.tick, kind.type_order(), ti, ei, kind));
            }
        }
    }
    all.sort_unstable_by_key(|m| (m.0, m.1, m.2, m.3));
    let mut out = Vec::new();
    let mut i = 0;
    while i < all.len() {
        let (tick, ty, track, _, _) = all[i];
        let mut chosen = all[i].4;
        let mut j = i;
        while j < all.len() && all[j].0 == tick && all[j].1 == ty {
            if all[j].2 == track {
                chosen = all[j].4;
            }
            j += 1;
        }
        out.push(MetaEvent { tick, kind: chosen });
        i = j;
    }
    out
}

/// Приводит песню к инвариантам после ручных правок: ноты отсортированы, на одной клавише
/// не перекрываются, длина не меньше тика; контроллеры отсортированы по тику.
pub fn normalize_song(song: &mut Song) {
    let mut stats = BuildStats::default();
    for c in &mut song.channels {
        for n in &mut c.notes {
            if n.end <= n.start {
                n.end = n.start + 1;
            }
        }
        c.notes = resolve_overlaps(std::mem::take(&mut c.notes), &mut stats);
        c.controls.sort_by_key(|x| x.tick);
    }
    song.meta.sort_by_key(|m| m.tick);
    song.sysex.sort_by_key(|x| x.tick);
}

pub fn decode_name(bytes: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1251.decode_without_bom_handling(bytes).0.into_owned(),
    };
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0').to_string();
    (!text.is_empty()).then_some(text)
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ControlKind, Division, MetaKind};
    use crate::read::read_midi;
    use crate::testkit::{SmfBuilder, TrackBuilder};

    fn build(b: SmfBuilder) -> (Song, BuildStats) {
        build_song(&read_midi(&b.bytes()).unwrap())
    }

    fn n(start: u64, end: u64, key: u8, velocity: u8) -> Note {
        Note { start, end, key, velocity }
    }

    #[test]
    fn pairs_simple_notes() {
        let (song, stats) = build(SmfBuilder::new(96).track(TrackBuilder::new().note(0, 10, 0, 60, 100).note(10, 20, 0, 62, 90)));
        assert_eq!(song.channels[0].notes, vec![n(0, 10, 60, 100), n(10, 20, 62, 90)]);
        assert_eq!(stats, BuildStats::default());
    }

    #[test]
    fn new_note_on_written_before_old_note_off_on_same_tick() {
        // Частый случай экспорта: на тике 10 сначала note_on новой ноты, потом note_off старой.
        let t = TrackBuilder::new().on(0, 0, 60, 100).on(10, 0, 60, 80).off(10, 0, 60).off(20, 0, 60);
        let (song, stats) = build(SmfBuilder::new(96).track(t));
        assert_eq!(song.channels[0].notes, vec![n(0, 10, 60, 100), n(10, 20, 60, 80)]);
        assert_eq!(stats.orphan_offs, 0);
        assert_eq!(stats.overlaps_resolved, 0);
    }

    #[test]
    fn orphan_offs_are_dropped() {
        let t = TrackBuilder::new().off(0, 0, 60).note(5, 10, 0, 60, 100).off(12, 0, 60).off(13, 0, 61);
        let (song, stats) = build(SmfBuilder::new(96).track(t));
        assert_eq!(song.channels[0].notes, vec![n(5, 10, 60, 100)]);
        assert_eq!(stats.orphan_offs, 3);
    }

    #[test]
    fn overlapping_same_key_is_split() {
        // Вторая нота начинается, пока первая ещё звучит (разные дорожки, один канал).
        let a = TrackBuilder::new().note(0, 100, 0, 60, 100);
        let b = TrackBuilder::new().note(50, 150, 0, 60, 90);
        let (song, stats) = build(SmfBuilder::new(96).track(a).track(b));
        // FIFO: off@100 закрывает ноту с 0, off@150 — ноту с 50. Затем первая обрезается до 50.
        assert_eq!(song.channels[0].notes, vec![n(0, 50, 60, 100), n(50, 150, 60, 90)]);
        assert_eq!(stats.overlaps_resolved, 1);
    }

    #[test]
    fn same_start_duplicates_merge() {
        let a = TrackBuilder::new().note(0, 40, 0, 60, 70);
        let b = TrackBuilder::new().note(0, 90, 0, 60, 110);
        let (song, stats) = build(SmfBuilder::new(96).track(a).track(b));
        assert_eq!(song.channels[0].notes, vec![n(0, 90, 60, 110)]);
        assert_eq!(stats.same_start_merged, 1);
    }

    #[test]
    fn hanging_notes_close_at_song_end() {
        let t = TrackBuilder::new().on(0, 0, 60, 100).note(10, 300, 1, 40, 50);
        let (song, stats) = build(SmfBuilder::new(96).track(t));
        assert_eq!(song.channels[0].notes, vec![n(0, 300, 60, 100)]);
        assert_eq!(stats.hanging_closed, 1);
    }

    #[test]
    fn hanging_note_at_last_tick_gets_a_quarter() {
        let t = TrackBuilder::new().note(0, 10, 0, 50, 100).on(10, 0, 60, 100);
        let (song, _) = build(SmfBuilder::new(96).track(t));
        assert!(song.channels[0].notes.contains(&n(10, 106, 60, 100)));
    }

    #[test]
    fn zero_length_drum_hit_survives() {
        let t = TrackBuilder::new().on(0, 9, 36, 100).off(0, 9, 36).note(0, 10, 0, 60, 100);
        let (song, stats) = build(SmfBuilder::new(96).track(t));
        assert_eq!(song.channels[9].notes, vec![n(0, 1, 36, 100)]);
        assert_eq!(stats.zero_length_extended, 1);
    }

    #[test]
    fn tempo_from_any_track_and_priority() {
        let a = TrackBuilder::new().note(0, 10, 0, 60, 100);
        let b = TrackBuilder::new().tempo(0, 400_000).tempo(96, 300_000);
        let c = TrackBuilder::new().tempo(96, 999_999);
        let (song, _) = build(SmfBuilder::new(96).track(a).track(b).track(c));
        assert_eq!(
            song.meta,
            vec![
                MetaEvent { tick: 0, kind: MetaKind::Tempo(400_000) },
                MetaEvent { tick: 96, kind: MetaKind::Tempo(300_000) },
            ]
        );
    }

    #[test]
    fn channel_names_from_tracks() {
        let a = TrackBuilder::new().name("Lead").note(0, 10, 0, 60, 100);
        let b = TrackBuilder::new().name("Lead").note(0, 10, 0, 64, 100);
        let c = TrackBuilder::new().name_bytes(&[0xCF, 0xE8, 0xE0, 0xED, 0xE8, 0xED, 0xEE]).note(0, 10, 1, 60, 100);
        let long = "x".repeat(40) + "ё".repeat(40).as_str();
        let d = TrackBuilder::new().name(&long).note(0, 10, 2, 60, 100);
        let (song, _) = build(SmfBuilder::new(96).track(a).track(b).track(c).track(d));
        assert_eq!(song.channels[0].name.as_deref(), Some("Lead"));
        assert_eq!(song.channels[1].name.as_deref(), Some("Пианино"));
        assert_eq!(song.channels[2].name.as_ref().unwrap().chars().count(), 64);
        assert_eq!(song.channel_label(3), "Channel 4");
        assert_eq!(song.channel_label(9), "Drums");
    }

    #[test]
    fn controls_and_division_kept() {
        let t = TrackBuilder::new().program(0, 3, 40).cc(5, 3, 7, 100).bend(6, 3, 9000).note(0, 10, 3, 60, 100);
        let (song, _) = build(SmfBuilder::new(120).track(t));
        assert_eq!(song.division, Division::Ticks(120));
        let kinds: Vec<_> = song.channels[3].controls.iter().map(|c| (c.tick, c.kind, c.value)).collect();
        assert_eq!(
            kinds,
            vec![(0, ControlKind::Program, 40), (5, ControlKind::Cc(7), 100), (6, ControlKind::PitchBend, 9000)]
        );
        assert_eq!(song.channel_label(3), "Violin");
    }
}
