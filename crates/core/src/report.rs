//! Отчёты об анализе и обработке (они же — JSON для `--json`).

use serde::Serialize;

use crate::analysis::{Metrics, TimeRange};
use crate::build::BuildStats;
use crate::gm::gm_name;
use crate::limits::Status;
use crate::model::{Song, DRUM_CHANNEL};
use crate::passes::cleanup::CleanupStats;
use crate::passes::limiter::RemovedNote;
use crate::passes::trim::TrimStats;
use crate::passes::velocity::VelocityStats;
use crate::pipeline::{Preset, Settings};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelInfo {
    /// 1–16.
    pub channel: u8,
    pub name: String,
    pub instrument: Option<String>,
    pub drums: bool,
    pub notes: usize,
    pub key_range: Option<(u8, u8)>,
    pub velocity_range: Option<(u8, u8)>,
}

pub fn channel_infos(song: &Song) -> Vec<ChannelInfo> {
    song.channels
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_empty())
        .map(|(ch, c)| {
            let range = |f: fn(&crate::model::Note) -> u8| {
                let min = c.notes.iter().map(f).min()?;
                Some((min, c.notes.iter().map(f).max()?))
            };
            ChannelInfo {
                channel: ch as u8 + 1,
                name: song.channel_label(ch),
                instrument: if ch == DRUM_CHANNEL { None } else { c.first_program().map(|p| gm_name(p).to_string()) },
                drums: ch == DRUM_CHANNEL,
                notes: c.notes.len(),
                key_range: range(|n| n.key),
                velocity_range: range(|n| n.velocity),
            }
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Changes {
    pub build: BuildStats,
    pub trim: TrimStats,
    pub velocity: Option<VelocityStats>,
    pub cleanup: Option<CleanupStats>,
    pub removed_notes: Vec<RemovedNote>,
    pub removed_share: f64,
    /// Доля удалённых нот без учёта унисонов и микронот.
    pub audible_share: f64,
    pub heavily_simplified: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FileReport {
    pub input: Option<String>,
    pub output: Option<String>,
    pub preset: Preset,
    pub settings: Settings,
    pub status: Status,
    pub recovered: bool,
    pub problems: Vec<String>,
    pub before: Metrics,
    pub after: Metrics,
    pub channels: Vec<ChannelInfo>,
    pub changes: Changes,
    pub overloaded_ranges: Vec<TimeRange>,
    pub timeline_before: Vec<u32>,
    pub timeline_after: Vec<u32>,
    pub processing_ms: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnalysisReport {
    pub input: Option<String>,
    pub status: Status,
    pub recovered: bool,
    pub problems: Vec<String>,
    pub metrics: Metrics,
    pub channels: Vec<ChannelInfo>,
    /// Что исправится при обработке: пустые note_off, наложения и т. д.
    pub issues: BuildStats,
    pub overloaded_ranges: Vec<TimeRange>,
    pub timeline: Vec<u32>,
}
