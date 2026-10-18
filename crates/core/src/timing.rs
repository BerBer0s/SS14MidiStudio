//! Перевод тиков в реальное время по карте темпа.

use crate::model::{Division, MetaEvent, MetaKind};

pub const DEFAULT_TEMPO: u32 = 500_000;

#[derive(Clone, Copy, Debug)]
struct Segment {
    tick: u64,
    us: f64,
    us_per_tick: f64,
}

#[derive(Clone, Debug)]
pub struct TempoMap {
    segments: Vec<Segment>,
}

impl TempoMap {
    /// `tempos` — пары (тик, микросекунд на четверть), в любом порядке.
    pub fn new(division: Division, tempos: impl IntoIterator<Item = (u64, u32)>) -> TempoMap {
        match division {
            Division::Smpte { fps, ticks_per_frame } => {
                let fps = if fps == 29 { 29.97 } else { fps as f64 };
                let per_second = (fps * ticks_per_frame.max(1) as f64).max(1.0);
                TempoMap { segments: vec![Segment { tick: 0, us: 0.0, us_per_tick: 1e6 / per_second }] }
            }
            Division::Ticks(tpq) => {
                let tpq = tpq.max(1) as f64;
                let mut list: Vec<(u64, u32)> = tempos.into_iter().collect();
                list.sort_by_key(|&(t, _)| t);
                let mut segments = vec![Segment { tick: 0, us: 0.0, us_per_tick: DEFAULT_TEMPO as f64 / tpq }];
                for (tick, tempo) in list {
                    let tempo = tempo.max(1) as f64;
                    let last = *segments.last().unwrap();
                    if tick == last.tick {
                        segments.last_mut().unwrap().us_per_tick = tempo / tpq;
                    } else {
                        let us = last.us + (tick - last.tick) as f64 * last.us_per_tick;
                        segments.push(Segment { tick, us, us_per_tick: tempo / tpq });
                    }
                }
                TempoMap { segments }
            }
        }
    }

    pub fn from_meta(division: Division, meta: &[MetaEvent]) -> TempoMap {
        TempoMap::new(
            division,
            meta.iter().filter_map(|m| match m.kind {
                MetaKind::Tempo(t) => Some((m.tick, t)),
                _ => None,
            }),
        )
    }

    fn segment_for_tick(&self, tick: u64) -> &Segment {
        let i = self.segments.partition_point(|s| s.tick <= tick);
        &self.segments[i.saturating_sub(1)]
    }

    pub fn tick_to_us_f(&self, tick: u64) -> f64 {
        let s = self.segment_for_tick(tick);
        s.us + (tick - s.tick) as f64 * s.us_per_tick
    }

    /// Время тика в целых микросекундах.
    pub fn tick_to_us(&self, tick: u64) -> u64 {
        self.tick_to_us_f(tick).round() as u64
    }

    pub fn tick_to_sec(&self, tick: u64) -> f64 {
        self.tick_to_us_f(tick) / 1e6
    }

    /// Наибольший тик, время которого не позже `us`.
    pub fn us_to_tick(&self, us: f64) -> u64 {
        let i = self.segments.partition_point(|s| s.us <= us);
        let s = &self.segments[i.saturating_sub(1)];
        let ticks = ((us - s.us).max(0.0) / s.us_per_tick + 1e-9).floor() as u64;
        let mut tick = s.tick + ticks;
        // Защита от погрешности округления в обе стороны.
        while tick > 0 && self.tick_to_us_f(tick) > us + 1e-6 {
            tick -= 1;
        }
        while self.tick_to_us_f(tick + 1) <= us + 1e-6 {
            tick += 1;
        }
        tick
    }

    pub fn us_per_tick_at(&self, tick: u64) -> f64 {
        self.segment_for_tick(tick).us_per_tick
    }

    /// Темп (микросекунд на четверть) в заданном тике для отображения BPM.
    pub fn bpm_at(&self, tick: u64, division: Division) -> f64 {
        match division {
            Division::Ticks(tpq) => 60e6 / (self.us_per_tick_at(tick) * tpq.max(1) as f64),
            Division::Smpte { .. } => 120.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tempo_is_120_bpm() {
        let m = TempoMap::new(Division::Ticks(480), []);
        assert_eq!(m.tick_to_us(480), 500_000);
        assert_eq!(m.tick_to_us(960), 1_000_000);
    }

    #[test]
    fn tempo_changes_accumulate() {
        // 120 BPM первые 480 тиков, затем 60 BPM.
        let m = TempoMap::new(Division::Ticks(480), [(480, 1_000_000), (0, 500_000)]);
        assert_eq!(m.tick_to_us(480), 500_000);
        assert_eq!(m.tick_to_us(960), 1_500_000);
        assert_eq!(m.us_to_tick(1_500_000.0), 960);
        assert_eq!(m.us_to_tick(1_499_999.0), 959);
    }

    #[test]
    fn smpte_ignores_tempo() {
        let m = TempoMap::new(Division::Smpte { fps: 25, ticks_per_frame: 40 }, [(0, 1)]);
        assert_eq!(m.tick_to_us(1000), 1_000_000);
    }

    #[test]
    fn same_tick_tempo_replaces() {
        let m = TempoMap::new(Division::Ticks(100), [(0, 500_000), (0, 1_000_000)]);
        assert_eq!(m.tick_to_us(100), 1_000_000);
    }
}
