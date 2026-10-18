//! Шаг 3: громкость нот (velocity).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::model::{Song, DRUM_CHANNEL};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VelocityMode {
    /// Один множитель на все мелодические каналы — баланс микса сохраняется.
    Song,
    /// Свой множитель на каждый канал (как в старой версии).
    PerChannel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VelocitySettings {
    pub drum_cap: u8,
    pub melodic_cap: u8,
    pub mode: VelocityMode,
    pub chord_softening: bool,
}

impl Default for VelocitySettings {
    fn default() -> VelocitySettings {
        VelocitySettings { drum_cap: 85, melodic_cap: 127, mode: VelocityMode::Song, chord_softening: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct VelocityStats {
    pub drum_factor: Option<f64>,
    /// (канал 1–16, множитель)
    pub melodic_factors: Vec<(u8, f64)>,
    pub softened_notes: u32,
}

fn scale(v: u8, factor: f64) -> u8 {
    (v as f64 * factor).round().clamp(1.0, 127.0) as u8
}

pub fn apply_velocity(song: &mut Song, s: &VelocitySettings) -> VelocityStats {
    let mut stats = VelocityStats::default();
    if s.chord_softening {
        for (ch, channel) in song.channels.iter_mut().enumerate() {
            if ch == DRUM_CHANNEL {
                continue;
            }
            // Ноты отсортированы по началу; кучу концов чистим до текущего начала (note off раньше note on).
            let mut ends: BinaryHeap<Reverse<u64>> = BinaryHeap::new();
            for n in &mut channel.notes {
                while ends.peek().is_some_and(|Reverse(e)| *e <= n.start) {
                    ends.pop();
                }
                ends.push(Reverse(n.end));
                let count = ends.len();
                if count >= 3 {
                    n.velocity = scale(n.velocity, 1.0 / (count as f64).sqrt());
                    stats.softened_notes += 1;
                }
            }
        }
    }

    let drums = &mut song.channels[DRUM_CHANNEL].notes;
    if let Some(max) = drums.iter().map(|n| n.velocity).max() {
        let factor = s.drum_cap.max(1) as f64 / max as f64;
        for n in drums.iter_mut() {
            n.velocity = scale(n.velocity, factor);
        }
        stats.drum_factor = Some(factor);
    }

    let melodic = |ch: usize| ch != DRUM_CHANNEL;
    match s.mode {
        VelocityMode::Song => {
            let max = song
                .channels
                .iter()
                .enumerate()
                .filter(|(ch, _)| melodic(*ch))
                .flat_map(|(_, c)| c.notes.iter().map(|n| n.velocity))
                .max();
            if let Some(max) = max {
                let factor = s.melodic_cap.max(1) as f64 / max as f64;
                for (ch, channel) in song.channels.iter_mut().enumerate() {
                    if melodic(ch) && !channel.notes.is_empty() {
                        channel.notes.iter_mut().for_each(|n| n.velocity = scale(n.velocity, factor));
                        stats.melodic_factors.push((ch as u8 + 1, factor));
                    }
                }
            }
        }
        VelocityMode::PerChannel => {
            for (ch, channel) in song.channels.iter_mut().enumerate() {
                if let (true, Some(max)) = (melodic(ch), channel.notes.iter().map(|n| n.velocity).max()) {
                    let factor = s.melodic_cap.max(1) as f64 / max as f64;
                    channel.notes.iter_mut().for_each(|n| n.velocity = scale(n.velocity, factor));
                    stats.melodic_factors.push((ch as u8 + 1, factor));
                }
            }
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Division, Note};

    fn song() -> Song {
        let mut s = Song::empty(Division::Ticks(96));
        let n = |start, key, velocity| Note { start, end: start + 10, key, velocity };
        s.channels[0].notes = vec![n(0, 60, 100), n(20, 62, 50)];
        s.channels[1].notes = vec![n(0, 40, 25)];
        s.channels[DRUM_CHANNEL].notes = vec![n(0, 36, 127), n(10, 38, 64)];
        s
    }

    fn vels(s: &Song, ch: usize) -> Vec<u8> {
        s.channels[ch].notes.iter().map(|n| n.velocity).collect()
    }

    #[test]
    fn drum_cap_limits_loud_drums() {
        let mut s = song();
        apply_velocity(&mut s, &VelocitySettings::default());
        assert_eq!(vels(&s, DRUM_CHANNEL), vec![85, 43]);
    }

    #[test]
    fn drum_cap_raises_quiet_drums() {
        let mut s = song();
        s.channels[DRUM_CHANNEL].notes.iter_mut().for_each(|n| n.velocity = 50);
        apply_velocity(&mut s, &VelocitySettings::default());
        assert_eq!(vels(&s, DRUM_CHANNEL), vec![85, 85]);
    }

    #[test]
    fn song_mode_keeps_balance() {
        let mut s = song();
        apply_velocity(&mut s, &VelocitySettings::default());
        assert_eq!(vels(&s, 0), vec![127, 64]);
        assert_eq!(vels(&s, 1), vec![32]);
    }

    #[test]
    fn per_channel_mode_maximizes_each() {
        let mut s = song();
        apply_velocity(&mut s, &VelocitySettings { mode: VelocityMode::PerChannel, ..Default::default() });
        assert_eq!(vels(&s, 1), vec![127]);
    }

    #[test]
    fn chord_softening() {
        let mut s = Song::empty(Division::Ticks(96));
        s.channels[0].notes = (0..4).map(|i| Note { start: 0, end: 10, key: 60 + i, velocity: 100 }).collect();
        let st = apply_velocity(&mut s, &VelocitySettings { chord_softening: true, melodic_cap: 127, ..Default::default() });
        assert_eq!(st.softened_notes, 2);
        // Третья нота: 100/√3 ≈ 58, четвёртая: 100/2 = 50, затем нормализация к 127 по максимуму 100.
        assert_eq!(vels(&s, 0), vec![127, 127, 74, 64]);
    }
}
