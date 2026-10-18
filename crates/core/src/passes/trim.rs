//! Шаг 2: обрезка тишины в начале и подготовка старта (громкость каналов, пре-ролл).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::limits::Limits;
use crate::model::{Control, ControlKind, MetaEvent, Song};

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TrimStats {
    pub trimmed_s: f64,
    pub preroll_ms: f64,
    pub volume_init_added: u32,
    pub setup_events: u32,
}

/// Сколько стартовых событий класть в один кадр.
pub const PREROLL_CHUNK: usize = 40;

pub fn trim_and_prepare(song: &mut Song, limits: &Limits) -> TrimStats {
    let mut stats = TrimStats::default();
    let Some(t0) = song.channels.iter().flat_map(|c| c.notes.iter().map(|n| n.start)).min() else {
        return stats;
    };
    stats.trimmed_s = song.tempo_map().tick_to_sec(t0);

    // 1. Всё до первой ноты (и на её тике) сворачивается в состояние на тике 0.
    let mut setup: Vec<(usize, Control)> = Vec::new();
    for (ch, channel) in song.channels.iter_mut().enumerate() {
        let mut state: BTreeMap<ControlKind, u16> = BTreeMap::new();
        let mut rest = Vec::with_capacity(channel.controls.len());
        for c in channel.controls.drain(..) {
            if c.tick <= t0 {
                state.insert(c.kind, c.value);
            } else {
                rest.push(Control { tick: c.tick - t0, ..c });
            }
        }
        // 2. Громкость каналов с нотами — на максимум, если файл не задал её сам.
        if !channel.notes.is_empty() {
            for cc in [7u8, 11] {
                if let std::collections::btree_map::Entry::Vacant(e) = state.entry(ControlKind::Cc(cc)) {
                    e.insert(127);
                    stats.volume_init_added += 1;
                }
            }
        }
        setup.extend(state.into_iter().map(|(kind, value)| (ch, Control { tick: 0, kind, value })));
        channel.controls = rest;
        for n in &mut channel.notes {
            n.start -= t0;
            n.end -= t0;
        }
    }
    let mut meta_state: BTreeMap<u8, MetaEvent> = BTreeMap::new();
    let mut meta_rest = Vec::new();
    for m in song.meta.drain(..) {
        if m.tick <= t0 {
            meta_state.insert(m.kind.type_order(), MetaEvent { tick: 0, ..m });
        } else {
            meta_rest.push(MetaEvent { tick: m.tick - t0, ..m });
        }
    }
    song.meta = meta_state.into_values().chain(meta_rest).collect();
    let mut setup_sysex = Vec::new();
    let mut sysex_rest = Vec::new();
    for mut s in song.sysex.drain(..) {
        if s.tick <= t0 {
            s.tick = 0;
            setup_sysex.push(s);
        } else {
            s.tick -= t0;
            sysex_rest.push(s);
        }
    }

    // 3. Пре-ролл: стартовые события порциями по PREROLL_CHUNK на кадр, музыка — после них.
    let total = setup.len() + setup_sysex.len();
    stats.setup_events = total as u32;
    if total == 0 {
        song.sysex = sysex_rest;
        return stats;
    }
    let map = song.tempo_map();
    let frame_ticks = (limits.frame_us() as f64 / map.us_per_tick_at(0)).floor() as u64 + 1;
    let chunks = total.div_ceil(PREROLL_CHUNK) as u64;
    let shift = chunks * frame_ticks;
    stats.preroll_ms = map.tick_to_us_f(shift) / 1000.0;

    for channel in &mut song.channels {
        for n in &mut channel.notes {
            n.start += shift;
            n.end += shift;
        }
        for c in &mut channel.controls {
            c.tick += shift;
        }
    }
    for m in &mut song.meta {
        if m.tick > 0 {
            m.tick += shift;
        }
    }
    for s in &mut sysex_rest {
        s.tick += shift;
    }
    for (i, s) in setup_sysex.iter_mut().enumerate() {
        s.tick = (i / PREROLL_CHUNK) as u64 * frame_ticks;
    }
    let offset = setup_sysex.len();
    // Порядок внутри старта: по каналу, затем инструмент, контроллеры, остальное.
    setup.sort_by_key(|(ch, c)| (*ch, c.kind));
    let mut placed: Vec<Vec<Control>> = vec![Vec::new(); song.channels.len()];
    for (i, (ch, c)) in setup.into_iter().enumerate() {
        let tick = ((i + offset) / PREROLL_CHUNK) as u64 * frame_ticks;
        placed[ch].push(Control { tick, ..c });
    }
    for (ch, mut list) in placed.into_iter().enumerate() {
        list.append(&mut song.channels[ch].controls);
        song.channels[ch].controls = list;
    }
    setup_sysex.append(&mut sysex_rest);
    song.sysex = setup_sysex;
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::measure;
    use crate::export::export_smf;
    use crate::model::{Division, MetaKind, Note, CHANNELS};
    use crate::read::read_midi;

    fn note(start: u64, end: u64, key: u8) -> Note {
        Note { start, end, key, velocity: 100 }
    }

    #[test]
    fn silence_and_early_controls_collapse() {
        let mut s = Song::empty(Division::Ticks(480));
        s.meta = vec![
            MetaEvent { tick: 0, kind: MetaKind::Tempo(500_000) },
            MetaEvent { tick: 100, kind: MetaKind::Tempo(400_000) },
            MetaEvent { tick: 2000, kind: MetaKind::Tempo(300_000) },
        ];
        s.channels[0].notes = vec![note(960, 1440, 60)];
        s.channels[0].controls = (0..500)
            .map(|i| Control { tick: i, kind: ControlKind::PitchBend, value: i as u16 })
            .chain([Control { tick: 960, kind: ControlKind::Cc(7), value: 90 }, Control { tick: 1000, kind: ControlKind::Cc(1), value: 5 }])
            .collect();
        let st = trim_and_prepare(&mut s, &Limits::default());
        let setup: Vec<_> = s.channels[0].controls.iter().filter(|c| c.tick == 0).map(|c| (c.kind, c.value)).collect();
        // Последний pitch bend, громкость из файла (90), expression добавлен.
        assert_eq!(setup, vec![(ControlKind::Cc(7), 90), (ControlKind::Cc(11), 127), (ControlKind::PitchBend, 499)]);
        assert_eq!(st.volume_init_added, 1);
        let first = s.channels[0].notes[0].start;
        assert!(first > 0, "пре-ролл должен отодвинуть первую ноту");
        assert!(st.preroll_ms > 16.0 && st.preroll_ms < 25.0, "{}", st.preroll_ms);
        assert_eq!(s.channels[0].notes[0].end - first, 480);
        // Темп: состояние на 0 (последний до первой ноты), дальнейшие смены сдвинуты.
        assert_eq!(s.meta[0], MetaEvent { tick: 0, kind: MetaKind::Tempo(400_000) });
        assert_eq!(s.meta[1].tick, 2000 - 960 + first);
        assert_eq!(s.channels[0].controls.last().unwrap().tick, 1000 - 960 + first);
    }

    #[test]
    fn sixteen_channels_setup_fits_frames() {
        let mut s = Song::empty(Division::Ticks(480));
        for ch in 0..CHANNELS {
            s.channels[ch].notes = vec![note(0, 480, 60)];
            s.channels[ch].controls = vec![
                Control { tick: 0, kind: ControlKind::Program, value: 1 },
                Control { tick: 0, kind: ControlKind::Cc(10), value: 64 },
            ];
        }
        trim_and_prepare(&mut s, &Limits::default());
        let m = measure(&read_midi(&export_smf(&s)).unwrap(), &Limits::default()).metrics;
        assert!(m.peak_per_frame.value <= 60, "{}", m.peak_per_frame.value);
    }

    #[test]
    fn empty_song_untouched() {
        let mut s = Song::empty(Division::Ticks(96));
        assert_eq!(trim_and_prepare(&mut s, &Limits::default()), TrimStats::default());
    }
}
