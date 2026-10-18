//! Лимиты MIDI-событий SS14 (CVars `midi.*` в space-wizards/space-station-14).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// `midi.max_events_per_second` на сервере.
    pub max_per_second: u32,
    /// Цель оптимизатора — с запасом от `max_per_second`.
    pub target_per_second: u32,
    /// `midi.max_events_per_batch` — событий за кадр клиента.
    pub frame_limit: u32,
    /// Длина «кадра» для проверки, мс.
    pub frame_ms: f64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_per_second: 1000, target_per_second: 900, frame_limit: 60, frame_ms: 1000.0 / 60.0 }
    }
}

impl Limits {
    pub const SECOND_US: u64 = 1_000_000;

    pub fn frame_us(&self) -> u64 {
        (self.frame_ms * 1000.0).round().max(1.0) as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Over,
}
