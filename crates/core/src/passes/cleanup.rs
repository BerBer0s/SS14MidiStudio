//! Шаг 4: уплотнение без потерь — повторы, неиспользуемые события, прореживание кривых,
//! перенос note_off из перегруженных кадров.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::limits::Limits;
use crate::model::{ControlKind, Song};
use crate::timing::TempoMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CleanupSettings {
    pub ignored_ccs: Vec<u8>,
    pub cc_min_interval_ms: f64,
    pub cc_pause_ms: f64,
}

/// Неопределённые в MIDI 1.0 и General Purpose контроллеры: синтезатор SS14 их не использует.
pub fn default_ignored_ccs() -> Vec<u8> {
    let mut v: Vec<u8> = vec![3, 9, 14, 15];
    v.extend(20..=31);
    v.extend(85..=90);
    v.extend(102..=119);
    v.extend(16..=19);
    v.extend(80..=83);
    v.sort_unstable();
    v
}

impl Default for CleanupSettings {
    fn default() -> CleanupSettings {
        CleanupSettings { ignored_ccs: default_ignored_ccs(), cc_min_interval_ms: 20.0, cc_pause_ms: 100.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CleanupStats {
    pub unused_removed: u32,
    pub redundant_removed: u32,
    pub thinned: u32,
    pub offs_shifted: u32,
}

pub fn cleanup(song: &mut Song, s: &CleanupSettings, limits: &Limits) -> CleanupStats {
    let mut stats = CleanupStats::default();
    stats.unused_removed += song.sysex.len() as u32;
    song.sysex.clear();
    for channel in &mut song.channels {
        let before = channel.controls.len();
        channel.controls.retain(|c| match c.kind {
            ControlKind::PolyPressure(_) => false,
            ControlKind::Cc(n) => !s.ignored_ccs.contains(&n),
            _ => true,
        });
        stats.unused_removed += (before - channel.controls.len()) as u32;
    }
    stats.redundant_removed += remove_redundant(song);
    let map = song.tempo_map();
    stats.thinned += thin_curves(song, s, &map);
    stats.redundant_removed += remove_redundant(song);
    stats.offs_shifted += shift_releases(song, limits, &map);
    stats
}

/// Удаляет события, которые повторяют текущее значение того же параметра канала.
pub fn remove_redundant(song: &mut Song) -> u32 {
    let mut removed = 0;
    for channel in &mut song.channels {
        let mut state: HashMap<ControlKind, u16> = HashMap::new();
        let before = channel.controls.len();
        channel.controls.retain(|c| state.insert(c.kind, c.value) != Some(c.value));
        removed += (before - channel.controls.len()) as u32;
    }
    removed
}

fn is_continuous(kind: ControlKind) -> bool {
    match kind {
        ControlKind::PitchBend | ControlKind::ChannelPressure => true,
        ControlKind::Cc(n) => !matches!(n, 0 | 32 | 6 | 38 | 96..=101 | 64..=69 | 120..=127),
        _ => false,
    }
}

/// Прореживание плавных кривых: не чаще одного события в `cc_min_interval_ms` на параметр,
/// с сохранением первого, последнего и последнего перед паузой.
fn thin_curves(song: &mut Song, s: &CleanupSettings, map: &TempoMap) -> u32 {
    let min_gap = (s.cc_min_interval_ms * 1000.0) as u64;
    let pause = (s.cc_pause_ms * 1000.0) as u64;
    let mut thinned = 0;
    for channel in &mut song.channels {
        let mut streams: HashMap<ControlKind, Vec<usize>> = HashMap::new();
        for (i, c) in channel.controls.iter().enumerate() {
            if is_continuous(c.kind) {
                streams.entry(c.kind).or_default().push(i);
            }
        }
        let mut keep = vec![true; channel.controls.len()];
        for idx in streams.values() {
            let times: Vec<u64> = idx.iter().map(|&i| map.tick_to_us(channel.controls[i].tick)).collect();
            let mut last_kept = times[0];
            for k in 1..idx.len() {
                let last = k + 1 == idx.len();
                let before_pause = !last && times[k + 1] - times[k] >= pause;
                if last || before_pause || times[k] - last_kept >= min_gap {
                    last_kept = times[k];
                } else {
                    keep[idx[k]] = false;
                    thinned += 1;
                }
            }
        }
        let mut it = keep.iter();
        channel.controls.retain(|_| *it.next().unwrap());
    }
    thinned
}

#[derive(Clone, Copy)]
enum EvKind {
    End(u32),
    Other,
    Start,
}

/// Перенос концов нот на кадр раньше там, где кадр перегружен. Звучит неотличимо,
/// а событий в перегруженном кадре становится меньше.
fn shift_releases(song: &mut Song, limits: &Limits, map: &TempoMap) -> u32 {
    let frame = limits.frame_us();
    let limit = limits.frame_limit as usize;
    // Ноты списком (канал, индекс), события по времени в порядке экспорта.
    let mut refs: Vec<(usize, usize)> = Vec::new();
    let mut events: Vec<(u64, u8, u64, EvKind)> = Vec::new(); // (тик, класс, мкс, вид)
    for (ch, c) in song.channels.iter().enumerate() {
        for (i, n) in c.notes.iter().enumerate() {
            let id = refs.len() as u32;
            refs.push((ch, i));
            events.push((n.end, 0, map.tick_to_us(n.end), EvKind::End(id)));
            events.push((n.start, 2, map.tick_to_us(n.start), EvKind::Start));
        }
        for ctl in &c.controls {
            events.push((ctl.tick, 1, map.tick_to_us(ctl.tick), EvKind::Other));
        }
    }
    if events.len() <= limit {
        return 0;
    }
    events.sort_unstable_by_key(|e| (e.0, e.1));
    let mut multiset: BTreeMap<u64, u32> = BTreeMap::new();
    for e in &events {
        *multiset.entry(e.2).or_default() += 1;
    }
    let mut moved = vec![false; events.len()];
    let mut shifted = 0;
    let mut lo = 0;
    let mut count = 0usize;
    for j in 0..events.len() {
        let tj = events[j].2;
        count += 1;
        while events[lo].2 + frame <= tj {
            if !moved[lo] {
                count -= 1;
            }
            lo += 1;
        }
        if count <= limit {
            continue;
        }
        // Окно (tj - frame, tj]. Цель переноса — не позже начала окна.
        let boundary = tj.saturating_sub(frame);
        let target_tick = map.us_to_tick(boundary as f64);
        let target_us = map.tick_to_us(target_tick);
        let mut candidates: Vec<usize> =
            (lo..=j).filter(|&k| !moved[k] && matches!(events[k].3, EvKind::End(_))).collect();
        candidates.sort_by_key(|&k| events[k].2);
        for k in candidates {
            if count <= limit {
                break;
            }
            let EvKind::End(id) = events[k].3 else { continue };
            let (ch, i) = refs[id as usize];
            let note = song.channels[ch].notes[i];
            if target_tick <= note.start || target_us > boundary || (target_tick - note.start) * 2 < note.len() {
                continue;
            }
            // Окрестность новой точки (2 кадра) не должна переполниться — проверка с запасом.
            let near: u32 = multiset.range(target_us.saturating_sub(frame)..target_us + frame).map(|(_, c)| c).sum();
            if near as usize + 1 > limit {
                continue;
            }
            let old = events[k].2;
            if let Some(c) = multiset.get_mut(&old) {
                *c -= 1;
                if *c == 0 {
                    multiset.remove(&old);
                }
            }
            *multiset.entry(target_us).or_default() += 1;
            song.channels[ch].notes[i].end = target_tick;
            moved[k] = true;
            count -= 1;
            shifted += 1;
        }
    }
    shifted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::measure;
    use crate::export::export_smf;
    use crate::model::{Control, Division, Note, SysEx};
    use crate::read::read_midi;

    fn ctl(tick: u64, kind: ControlKind, value: u16) -> Control {
        Control { tick, kind, value }
    }

    #[test]
    fn removes_unused_and_redundant() {
        let mut s = Song::empty(Division::Ticks(480));
        s.sysex.push(SysEx { tick: 0, data: vec![1, 2, 0xF7] });
        s.channels[0].controls = vec![
            ctl(0, ControlKind::Cc(7), 100),
            ctl(10, ControlKind::Cc(7), 100),
            ctl(20, ControlKind::Cc(80), 1),
            ctl(30, ControlKind::PolyPressure(60), 5),
            ctl(40, ControlKind::Program, 3),
            ctl(50, ControlKind::Program, 3),
            ctl(60, ControlKind::Cc(7), 90),
        ];
        let st = cleanup(&mut s, &CleanupSettings::default(), &Limits::default());
        assert_eq!(st.unused_removed, 3);
        assert_eq!(st.redundant_removed, 2);
        let left: Vec<_> = s.channels[0].controls.iter().map(|c| c.tick).collect();
        assert_eq!(left, vec![0, 40, 60]);
    }

    #[test]
    fn thins_pitch_bend_keeping_edges() {
        let mut s = Song::empty(Division::Ticks(480)); // 120 BPM: 1 тик ≈ 1.04 мс
        // Изгиб: 100 событий подряд каждый тик, потом пауза, потом одно событие.
        s.channels[0].controls = (0..100).map(|i| ctl(i, ControlKind::PitchBend, 8192 + i as u16)).collect();
        s.channels[0].controls.push(ctl(1000, ControlKind::PitchBend, 8192));
        let st = cleanup(&mut s, &CleanupSettings::default(), &Limits::default());
        let ticks: Vec<u64> = s.channels[0].controls.iter().map(|c| c.tick).collect();
        assert!(st.thinned > 80, "{}", st.thinned);
        assert_eq!(ticks[0], 0);
        assert!(ticks.contains(&99), "последнее перед паузой сохраняется");
        assert_eq!(*ticks.last().unwrap(), 1000);
    }

    #[test]
    fn switches_are_not_thinned() {
        let mut s = Song::empty(Division::Ticks(480));
        s.channels[0].controls = (0..10).map(|i| ctl(i, ControlKind::Cc(64), if i % 2 == 0 { 127 } else { 0 })).collect();
        let st = cleanup(&mut s, &CleanupSettings::default(), &Limits::default());
        assert_eq!(st.thinned, 0);
        assert_eq!(s.channels[0].controls.len(), 10);
    }

    #[test]
    fn release_shift_relieves_frame_burst() {
        // 40 нот кончаются на тике 960 и 40 новых начинаются там же: 80 событий в одном кадре.
        let mut s = Song::empty(Division::Ticks(480));
        for k in 0..40u8 {
            s.channels[(k % 8) as usize].notes.push(Note { start: 0, end: 960, key: 30 + k, velocity: 100 });
            s.channels[(k % 8) as usize].notes.push(Note { start: 960, end: 1920, key: 30 + k, velocity: 100 });
        }
        for c in &mut s.channels {
            c.notes.sort_by_key(|n| (n.start, n.key));
        }
        let before = measure(&read_midi(&export_smf(&s)).unwrap(), &Limits::default()).metrics.peak_per_frame.value;
        let st = cleanup(&mut s, &CleanupSettings::default(), &Limits::default());
        let after = measure(&read_midi(&export_smf(&s)).unwrap(), &Limits::default()).metrics.peak_per_frame.value;
        assert_eq!(before, 80);
        assert!(st.offs_shifted >= 20, "{}", st.offs_shifted);
        assert!(after <= 60, "{after}");
        // Ноты укоротились не больше чем на кадр.
        assert!(s.channels.iter().flat_map(|c| &c.notes).all(|n| n.len() >= 940));
    }
}
