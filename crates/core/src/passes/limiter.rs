//! Шаг 5: ограничитель плотности. Удаляет по одной самой незаметной ноте в перегруженных
//! местах, пока не уложится в лимит. Мелодию (верхний голос) и бас (нижний) не трогает.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use serde::{Deserialize, Serialize};

use crate::analysis::TimeRange;
use crate::limits::Limits;
use crate::model::{Song, DRUM_CHANNEL};
use crate::timing::TempoMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimiterSettings {
    pub micro_note_ms: f64,
}

impl Default for LimiterSettings {
    fn default() -> LimiterSettings {
        LimiterSettings { micro_note_ms: 30.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Tier {
    /// Унисон между каналами.
    A,
    /// Микроноты.
    B,
    /// Удвоения в октаву.
    C,
    /// Внутренние голоса.
    D,
    /// Ударные.
    E,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RemovedNote {
    pub at_s: f64,
    /// Положение в тиках результата — чтобы показать и вернуть ноту в редакторе.
    pub start: u64,
    pub end: u64,
    /// 1–16.
    pub channel: u8,
    pub key: u8,
    pub velocity: u8,
    pub tier: Tier,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LimiterStats {
    pub removed: Vec<RemovedNote>,
    pub unresolved: Vec<TimeRange>,
}

pub fn limit_density(song: &mut Song, limits: &Limits, s: &LimiterSettings) -> LimiterStats {
    let mut stats = LimiterStats::default();
    let map = song.tempo_map();
    // Защита считается один раз, до удалений.
    let protected = protected_notes(song);
    pass(song, &map, &protected, Limits::SECOND_US, limits.target_per_second, s, &mut stats);
    pass(song, &map, &protected, limits.frame_us(), limits.frame_limit, s, &mut stats);
    stats.unresolved.sort_by(|a, b| a.from_s.total_cmp(&b.from_s));
    stats
}

type Protected = Vec<Vec<bool>>; // [канал][индекс ноты]

/// Нота защищена, если в момент её начала она самая высокая или самая низкая
/// среди всех звучащих мелодических нот.
fn protected_notes(song: &Song) -> Protected {
    let mut out: Protected = song.channels.iter().map(|c| vec![false; c.notes.len()]).collect();
    // События: (тик, 0 — конец / 1 — начало, канал, индекс)
    let mut ev: Vec<(u64, u8, usize, usize)> = Vec::new();
    for (ch, c) in song.channels.iter().enumerate() {
        if ch == DRUM_CHANNEL {
            continue;
        }
        for (i, n) in c.notes.iter().enumerate() {
            ev.push((n.start, 1, ch, i));
            ev.push((n.end, 0, ch, i));
        }
    }
    ev.sort_unstable();
    let mut sounding = [0u32; 128];
    let mut k = 0;
    while k < ev.len() {
        let tick = ev[k].0;
        let mut starts = Vec::new();
        while k < ev.len() && ev[k].0 == tick {
            let (_, kind, ch, i) = ev[k];
            let key = song.channels[ch].notes[i].key as usize;
            if kind == 0 {
                sounding[key] -= 1;
            } else {
                sounding[key] += 1;
                starts.push((ch, i));
            }
            k += 1;
        }
        if starts.is_empty() {
            continue;
        }
        let top = (0..128).rev().find(|&x| sounding[x] > 0);
        let bottom = (0..128).find(|&x| sounding[x] > 0);
        for (ch, i) in starts {
            let key = Some(song.channels[ch].notes[i].key as usize);
            if key == top || key == bottom {
                out[ch][i] = true;
            }
        }
    }
    out
}

struct Flat {
    ch: u8,
    idx: usize,
    start: u64,
    end: u64,
    key: u8,
    vel: u8,
    dur_us: u64,
    start_us: u64,
    drum: bool,
    protected: bool,
    start_ev: usize,
    end_ev: usize,
    unison: Option<usize>,
    octave_partners: Vec<usize>,
}

type Key = (Tier, u8, u64, Reverse<u8>, Reverse<u64>, usize);

#[allow(clippy::too_many_arguments)]
fn pass(
    song: &mut Song,
    map: &TempoMap,
    protected: &Protected,
    window: u64,
    limit: u32,
    s: &LimiterSettings,
    stats: &mut LimiterStats,
) {
    let limit = limit as usize;
    let mut notes: Vec<Flat> = Vec::new();
    // (тик, класс, мкс, нота или usize::MAX для прочих событий)
    let mut events: Vec<(u64, u8, u64, usize)> = Vec::new();
    for (ch, c) in song.channels.iter().enumerate() {
        for (i, n) in c.notes.iter().enumerate() {
            let id = notes.len();
            let start_us = map.tick_to_us(n.start);
            let end_us = map.tick_to_us(n.end);
            notes.push(Flat {
                ch: ch as u8,
                idx: i,
                start: n.start,
                end: n.end,
                key: n.key,
                vel: n.velocity,
                dur_us: end_us - start_us,
                start_us,
                drum: ch == DRUM_CHANNEL,
                protected: protected.get(ch).and_then(|p| p.get(i)).copied().unwrap_or(false),
                start_ev: 0,
                end_ev: 0,
                unison: None,
                octave_partners: Vec::new(),
            });
            events.push((n.end, 0, end_us, id));
            events.push((n.start, 2, start_us, id));
        }
        for ctl in &c.controls {
            events.push((ctl.tick, 1, map.tick_to_us(ctl.tick), usize::MAX));
        }
    }
    for x in &song.sysex {
        events.push((x.tick, 1, map.tick_to_us(x.tick), usize::MAX));
    }
    if events.len() <= limit {
        return;
    }
    events.sort_unstable_by_key(|e| (e.0, e.1, e.3));
    for (k, e) in events.iter().enumerate() {
        if e.3 != usize::MAX {
            if e.1 == 2 {
                notes[e.3].start_ev = k;
            } else {
                notes[e.3].end_ev = k;
            }
        }
    }

    // Группы унисонов и октавные пары среди мелодических нот с одинаковым началом.
    let mut by_start: HashMap<u64, Vec<usize>> = HashMap::new();
    for (id, n) in notes.iter().enumerate() {
        if !n.drum {
            by_start.entry(n.start).or_default().push(id);
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for ids in by_start.values() {
        if ids.len() < 2 {
            continue;
        }
        let mut by_key: HashMap<u8, Vec<usize>> = HashMap::new();
        for &id in ids {
            by_key.entry(notes[id].key).or_default().push(id);
        }
        for (_, members) in by_key.iter().filter(|(_, m)| m.len() > 1) {
            let g = groups.len();
            groups.push(members.clone());
            for &id in members {
                notes[id].unison = Some(g);
            }
        }
        for &a in ids {
            for &b in ids {
                let d = notes[a].key.abs_diff(notes[b].key);
                if a != b && (d == 12 || d == 24 || d == 36) {
                    notes[a].octave_partners.push(b);
                }
            }
        }
    }

    let micro = (s.micro_note_ms * 1000.0) as u64;
    let mut alive = vec![true; notes.len()];
    let tier_of = |id: usize, alive: &[bool]| -> Option<Tier> {
        let n = &notes[id];
        if n.drum {
            return Some(Tier::E);
        }
        if let Some(g) = n.unison {
            // Кандидат — любой живой участник, кроме самого громкого (при равенстве — с меньшим каналом).
            let living: Vec<usize> = groups[g].iter().copied().filter(|&m| alive[m]).collect();
            if living.len() >= 2 {
                let keeper = *living.iter().max_by_key(|&&m| (notes[m].vel, Reverse(notes[m].ch))).unwrap();
                if keeper != id {
                    return Some(Tier::A);
                }
            }
        }
        if n.protected {
            return None;
        }
        if n.dur_us < micro {
            return Some(Tier::B);
        }
        for &p in &n.octave_partners {
            if !alive[p] {
                continue;
            }
            let o = &notes[p];
            // Кандидат — незащищённая нота пары; если незащищены обе — более тихая
            // (при равенстве — с большим каналом, затем с большей клавишей).
            let weaker = (n.vel, Reverse(n.ch), Reverse(n.key)) < (o.vel, Reverse(o.ch), Reverse(o.key));
            if o.protected || weaker {
                return Some(Tier::C);
            }
        }
        Some(Tier::D)
    };
    let key_of = |id: usize, tier: Tier| -> Key {
        let n = &notes[id];
        (tier, n.vel, n.dur_us, Reverse(n.ch), Reverse(n.start), id)
    };

    let mut heap: BinaryHeap<Reverse<Key>> = BinaryHeap::new();
    let mut lo = 0usize;
    let mut count = 0usize;
    let mut open_range: Option<TimeRange> = None;
    for j in 0..events.len() {
        let (_, _, tj, id) = events[j];
        if id == usize::MAX || alive[id] {
            count += 1;
            if id != usize::MAX
                && let Some(t) = tier_of(id, &alive)
            {
                heap.push(Reverse(key_of(id, t)));
            }
        }
        while events[lo].2 + window <= tj {
            let lid = events[lo].3;
            if lid == usize::MAX || alive[lid] {
                count -= 1;
            }
            lo += 1;
        }
        let in_window = |k: usize| k >= lo && k <= j;
        while count > limit {
            let Some(Reverse(entry)) = heap.pop() else {
                // Убрать больше нечего: фиксируем неразрешённый участок.
                let from_s = events[lo].2 as f64 / 1e6;
                let to_s = tj as f64 / 1e6;
                match &mut open_range {
                    Some(r) if from_s <= r.to_s => {
                        r.to_s = to_s;
                        r.peak = r.peak.max(count as u32);
                    }
                    _ => {
                        if let Some(r) = open_range.take() {
                            stats.unresolved.push(r);
                        }
                        open_range = Some(TimeRange { from_s, to_s, peak: count as u32 });
                    }
                }
                break;
            };
            let id = entry.5;
            if !alive[id] || !(in_window(notes[id].start_ev) || in_window(notes[id].end_ev)) {
                continue;
            }
            let Some(t) = tier_of(id, &alive) else { continue };
            let fresh = key_of(id, t);
            if fresh != entry {
                heap.push(Reverse(fresh));
                continue;
            }
            alive[id] = false;
            count -= in_window(notes[id].start_ev) as usize + in_window(notes[id].end_ev) as usize;
            let n = &notes[id];
            stats.removed.push(RemovedNote {
                at_s: n.start_us as f64 / 1e6,
                start: n.start,
                end: n.end,
                channel: n.ch + 1,
                key: n.key,
                velocity: n.vel,
                tier: t,
            });
        }
    }
    if let Some(r) = open_range {
        stats.unresolved.push(r);
    }

    // Удаляем мёртвые ноты из каналов.
    let mut dead: Vec<Vec<bool>> = song.channels.iter().map(|c| vec![false; c.notes.len()]).collect();
    for (id, n) in notes.iter().enumerate() {
        if !alive[id] {
            dead[n.ch as usize][n.idx] = true;
        }
    }
    for (ch, c) in song.channels.iter_mut().enumerate() {
        let mut it = dead[ch].iter();
        c.notes.retain(|_| !*it.next().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::measure;
    use crate::export::export_smf;
    use crate::model::{Division, Note};
    use crate::read::read_midi;

    fn n(start: u64, end: u64, key: u8, velocity: u8) -> Note {
        Note { start, end, key, velocity }
    }

    fn peaks(s: &Song) -> (u32, u32) {
        let m = measure(&read_midi(&export_smf(s)).unwrap(), &Limits::default()).metrics;
        (m.peak_per_second.value, m.peak_per_frame.value)
    }

    /// Плотная песня: аккорд из 6 нот (12 событий) каждые ~10 мс — около 1150 событий в секунду.
    fn dense(chords: u64) -> Song {
        let mut s = Song::empty(Division::Ticks(480)); // 120 BPM, 1 тик ≈ 1.04 мс
        for c in 0..chords {
            let t = c * 10;
            for (k, key) in [48u8, 55, 60, 64, 67, 72].into_iter().enumerate() {
                s.channels[k % 3].notes.push(n(t, t + 8, key, 60 + k as u8 * 5));
            }
        }
        for c in &mut s.channels {
            c.notes.sort_by_key(|x| (x.start, x.key));
        }
        s
    }

    #[test]
    fn fits_target_and_keeps_melody_and_bass() {
        let mut s = dense(500);
        let (before, _) = peaks(&s);
        assert!(before > 900, "{before}");
        // Ноты в тесте короче 30 мс; порог микронот снижен, чтобы проверить октавы и голоса.
        let st = limit_density(&mut s, &Limits::default(), &LimiterSettings { micro_note_ms: 5.0 });
        let (after_s, after_f) = peaks(&s);
        assert!(after_s <= 900, "{after_s}");
        assert!(after_f <= 60, "{after_f}");
        assert!(st.unresolved.is_empty());
        let keys: Vec<u8> = s.channels.iter().flat_map(|c| c.notes.iter().map(|x| x.key)).collect();
        assert_eq!(keys.iter().filter(|&&k| k == 72).count(), 500, "мелодия цела");
        assert_eq!(keys.iter().filter(|&&k| k == 48).count(), 500, "бас цел");
        // Октавное удвоение C4 (60) к C5 (72) и C3 (48) уходит раньше внутренних голосов.
        assert!(st.removed.iter().take(10).all(|r| r.tier == Tier::C), "{:?}", &st.removed[..5]);
    }

    #[test]
    fn unison_goes_first_and_keeps_loudest() {
        let mut s = dense(500);
        // Дубль мелодии на другом канале — потише.
        let dup: Vec<Note> = s.channels[2].notes.iter().filter(|x| x.key == 72).map(|x| Note { velocity: 10, ..*x }).collect();
        s.channels[5].notes = dup;
        let st = limit_density(&mut s, &Limits::default(), &LimiterSettings::default());
        assert_eq!(st.removed[0].tier, Tier::A);
        assert_eq!(st.removed[0].channel, 6);
        assert!(s.channels[2].notes.iter().filter(|x| x.key == 72).count() == 500);
    }

    #[test]
    fn micro_notes_go_before_inner_voices() {
        let mut s = dense(500);
        let st = limit_density(&mut s, &Limits::default(), &LimiterSettings::default());
        assert!(st.removed.iter().all(|r| r.tier == Tier::B));
    }

    #[test]
    fn drums_are_last_resort() {
        let mut s = Song::empty(Division::Ticks(480));
        for c in 0..2000u64 {
            s.channels[DRUM_CHANNEL].notes.push(n(c, c + 1, 42, 40 + (c % 50) as u8));
        }
        let st = limit_density(&mut s, &Limits::default(), &LimiterSettings::default());
        assert!(st.removed.iter().all(|r| r.tier == Tier::E));
        assert!(peaks(&s).0 <= 900);
    }

    #[test]
    fn unresolved_when_only_protected_remain() {
        // 600 одиночных нот в секунду — каждая сама себе мелодия и бас.
        let mut s = Song::empty(Division::Ticks(480));
        for c in 0..1200u64 {
            s.channels[0].notes.push(n(c * 2, c * 2 + 1, 60 + (c % 12) as u8, 100));
        }
        let st = limit_density(&mut s, &Limits::default(), &LimiterSettings::default());
        assert!(st.removed.is_empty());
        assert!(!st.unresolved.is_empty());
    }

    #[test]
    fn untouched_when_within_limits() {
        let mut s = dense(10);
        let before = s.clone();
        let st = limit_density(&mut s, &Limits::default(), &LimiterSettings::default());
        assert!(st.removed.is_empty());
        assert_eq!(s, before);
    }
}
