//! Метрики плотности событий так, как их увидит SS14: все канальные события и SysEx.

use serde::Serialize;

use crate::limits::{Limits, Status};
use crate::model::CHANNELS;
use crate::read::{RawKind, RawSong};
use crate::timing::TempoMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Peak {
    pub value: u32,
    pub at_s: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Metrics {
    pub duration_s: f64,
    pub events_total: u64,
    pub note_on: u64,
    pub note_off: u64,
    pub controls: u64,
    pub sysex: u64,
    pub channels_used: u8,
    pub peak_per_second: Peak,
    pub peak_per_frame: Peak,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TimeRange {
    pub from_s: f64,
    pub to_s: f64,
    pub peak: u32,
}

/// Результат измерения файла: метрики, статус и отсортированные времена всех событий.
#[derive(Clone, Debug, Default)]
pub struct Measured {
    pub metrics: Metrics,
    pub times_us: Vec<u64>,
}

pub const TIMELINE_BUCKET_US: u64 = 100_000;

pub fn measure(raw: &RawSong, limits: &Limits) -> Measured {
    let tempos = raw.tracks.iter().flat_map(|t| t.events.iter()).filter_map(|e| match e.kind {
        RawKind::Meta(crate::model::MetaKind::Tempo(t)) => Some((e.tick, t)),
        _ => None,
    });
    let map = TempoMap::new(raw.division, tempos);
    let mut m = Metrics::default();
    let mut used = [false; CHANNELS];
    let mut times = Vec::new();
    for e in raw.tracks.iter().flat_map(|t| t.events.iter()) {
        let ch = match e.kind {
            RawKind::NoteOn { ch, .. } => {
                m.note_on += 1;
                Some(ch)
            }
            RawKind::NoteOff { ch, .. } => {
                m.note_off += 1;
                Some(ch)
            }
            RawKind::Control { ch, .. } => {
                m.controls += 1;
                Some(ch)
            }
            RawKind::SysEx(_) => {
                m.sysex += 1;
                None
            }
            _ => continue,
        };
        if let Some(ch) = ch {
            used[ch as usize] = true;
        }
        times.push(map.tick_to_us(e.tick));
    }
    times.sort_unstable();
    m.events_total = times.len() as u64;
    m.channels_used = used.iter().filter(|u| **u).count() as u8;
    m.duration_s = times.last().copied().unwrap_or(0) as f64 / 1e6;
    m.peak_per_second = peak(&times, Limits::SECOND_US);
    m.peak_per_frame = peak(&times, limits.frame_us());
    Measured { metrics: m, times_us: times }
}

/// Наибольшее число событий в окне [t, t + window) для любого t.
pub fn peak(times: &[u64], window_us: u64) -> Peak {
    let mut best = Peak::default();
    let mut hi = 0;
    for (lo, &t) in times.iter().enumerate() {
        if hi < lo {
            hi = lo;
        }
        while hi < times.len() && times[hi] < t + window_us {
            hi += 1;
        }
        let count = (hi - lo) as u32;
        if count > best.value {
            best = Peak { value: count, at_s: t as f64 / 1e6 };
        }
    }
    best
}

pub fn status_of(m: &Metrics, limits: &Limits) -> Status {
    let frame_ok = m.peak_per_frame.value <= limits.frame_limit;
    if frame_ok && m.peak_per_second.value <= limits.target_per_second {
        Status::Ok
    } else if frame_ok && m.peak_per_second.value <= limits.max_per_second {
        Status::Warn
    } else {
        Status::Over
    }
}

/// Число событий в каждом отрезке длиной `bucket_us`.
pub fn timeline(times: &[u64], bucket_us: u64) -> Vec<u32> {
    let Some(&last) = times.last() else { return Vec::new() };
    let mut out = vec![0u32; (last / bucket_us) as usize + 1];
    for &t in times {
        out[(t / bucket_us) as usize] += 1;
    }
    out
}

/// Участки, где в каком-то окне длиной `window_us` событий больше `limit`.
pub fn overloaded_ranges(times: &[u64], window_us: u64, limit: u32) -> Vec<TimeRange> {
    let mut out: Vec<TimeRange> = Vec::new();
    let mut hi = 0;
    for (lo, &t) in times.iter().enumerate() {
        if hi < lo {
            hi = lo;
        }
        while hi < times.len() && times[hi] < t + window_us {
            hi += 1;
        }
        let count = (hi - lo) as u32;
        if count <= limit {
            continue;
        }
        let from_s = t as f64 / 1e6;
        let to_s = times[hi - 1] as f64 / 1e6;
        match out.last_mut() {
            Some(r) if from_s <= r.to_s => {
                r.to_s = r.to_s.max(to_s);
                r.peak = r.peak.max(count);
            }
            _ => out.push(TimeRange { from_s, to_s, peak: count }),
        }
    }
    out
}

/// Участки, где превышена цель по секунде или лимит кадра — для отчёта.
pub fn problem_ranges(times: &[u64], limits: &Limits) -> Vec<TimeRange> {
    let mut all = overloaded_ranges(times, Limits::SECOND_US, limits.target_per_second);
    all.extend(overloaded_ranges(times, limits.frame_us(), limits.frame_limit));
    all.sort_by(|a, b| a.from_s.total_cmp(&b.from_s));
    let mut merged: Vec<TimeRange> = Vec::new();
    for r in all {
        match merged.last_mut() {
            Some(m) if r.from_s <= m.to_s => {
                m.to_s = m.to_s.max(r.to_s);
                m.peak = m.peak.max(r.peak);
            }
            _ => merged.push(r),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::read_midi;
    use crate::testkit::{SmfBuilder, TrackBuilder};

    #[test]
    fn peak_counts_half_open_window() {
        // Окно от 100 мкс: [100, 1_000_100) — 1_000_100 уже не входит.
        let times = [0, 100, 999_999, 1_000_000, 1_000_100];
        assert_eq!(peak(&times, 1_000_000).value, 3);
        assert_eq!(peak(&times, 1_000_001).value, 4);
        assert_eq!(peak(&[], 10).value, 0);
    }

    #[test]
    fn measure_counts_like_ss14() {
        // 120 BPM, 480 тиков = 0.5 с. Пустой note_off тоже считается.
        let t = TrackBuilder::new().note(0, 480, 0, 60, 100).off(480, 0, 61).cc(960, 1, 7, 100);
        let raw = read_midi(&SmfBuilder::new(480).track(t).bytes()).unwrap();
        let m = measure(&raw, &Limits::default()).metrics;
        assert_eq!((m.note_on, m.note_off, m.controls, m.events_total), (1, 2, 1, 4));
        assert_eq!(m.channels_used, 2);
        assert!((m.duration_s - 1.0).abs() < 1e-9);
        assert_eq!(m.peak_per_frame.value, 2);
    }

    #[test]
    fn status_thresholds() {
        let l = Limits::default();
        let mk = |s: u32, f: u32| Metrics {
            peak_per_second: Peak { value: s, at_s: 0.0 },
            peak_per_frame: Peak { value: f, at_s: 0.0 },
            ..Default::default()
        };
        assert_eq!(status_of(&mk(900, 60), &l), Status::Ok);
        assert_eq!(status_of(&mk(901, 60), &l), Status::Warn);
        assert_eq!(status_of(&mk(1000, 10), &l), Status::Warn);
        assert_eq!(status_of(&mk(1001, 10), &l), Status::Over);
        assert_eq!(status_of(&mk(10, 61), &l), Status::Over);
    }

    #[test]
    fn timeline_buckets() {
        assert_eq!(timeline(&[0, 50_000, 100_000, 250_000], 100_000), vec![2, 1, 1]);
    }

    #[test]
    fn ranges_merge() {
        let times: Vec<u64> = (0..100).map(|i| i * 1000).collect();
        let r = overloaded_ranges(&times, 20_000, 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].peak, 20);
    }
}
