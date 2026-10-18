//! Пресеты и порядок шагов обработки.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::analysis::{measure, problem_ranges, status_of, timeline, Measured, TIMELINE_BUCKET_US};
use crate::build::{build_song, BuildStats};
use crate::export::export_smf;
use crate::limits::Limits;
use crate::model::Song;
use crate::passes::cleanup::{cleanup, CleanupSettings, CleanupStats};
use crate::passes::limiter::{limit_density, LimiterSettings, LimiterStats, Tier};
use crate::passes::trim::{trim_and_prepare, TrimStats};
use crate::passes::velocity::{apply_velocity, VelocitySettings, VelocityStats};
use crate::read::{read_midi, ReadError};
use crate::report::{channel_infos, AnalysisReport, Changes, FileReport};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    /// Все шаги: файл гарантированно укладывается в лимиты SS14.
    Safe,
    /// Без удаления нот.
    Quality,
    /// Только починка, громкость и плотность не трогаются.
    Repair,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Safe, Preset::Quality, Preset::Repair];

    pub fn label_ru(self) -> &'static str {
        match self {
            Preset::Safe => "Безопасный для SS14",
            Preset::Quality => "Без потерь",
            Preset::Repair => "Только починка",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub preset: Preset,
    pub limits: Limits,
    pub velocity: VelocitySettings,
    pub cleanup: CleanupSettings,
    pub limiter: LimiterSettings,
    /// Доля слышимо удалённых нот (октавы, внутренние голоса, ударные), после которой
    /// файл помечается «сильно упрощён».
    pub heavy_share: f64,
}

impl Settings {
    pub fn new(preset: Preset) -> Settings {
        Settings {
            preset,
            limits: Limits::default(),
            velocity: VelocitySettings::default(),
            cleanup: CleanupSettings::default(),
            limiter: LimiterSettings::default(),
            heavy_share: 0.15,
        }
    }

    pub fn velocity_enabled(&self) -> bool {
        self.preset != Preset::Repair
    }

    pub fn cleanup_enabled(&self) -> bool {
        self.preset != Preset::Repair
    }

    pub fn limiter_enabled(&self) -> bool {
        self.preset == Preset::Safe
    }
}

impl Default for Settings {
    fn default() -> Settings {
        Settings::new(Preset::Safe)
    }
}

/// Прочитанный файл: модель песни, что было исправлено при сборке, и метрики «как есть».
#[derive(Clone, Debug)]
pub struct Loaded {
    pub song: Song,
    pub build: BuildStats,
    pub before: Measured,
}

pub fn load(bytes: &[u8], limits: &Limits) -> Result<Loaded, ReadError> {
    let raw = read_midi(bytes)?;
    let before = measure(&raw, limits);
    let (song, build) = build_song(&raw);
    Ok(Loaded { song, build, before })
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OptimizeStats {
    pub trim: TrimStats,
    pub velocity: Option<VelocityStats>,
    pub cleanup: Option<CleanupStats>,
    pub limiter: Option<LimiterStats>,
}

/// Шаги 2–5 над уже собранной песней.
pub fn optimize(song: &mut Song, settings: &Settings) -> OptimizeStats {
    let mut stats = OptimizeStats { trim: trim_and_prepare(song, &settings.limits), ..Default::default() };
    if settings.velocity_enabled() {
        stats.velocity = Some(apply_velocity(song, &settings.velocity));
    }
    if settings.cleanup_enabled() {
        stats.cleanup = Some(cleanup(song, &settings.cleanup, &settings.limits));
    }
    if settings.limiter_enabled() {
        stats.limiter = Some(limit_density(song, &settings.limits, &settings.limiter));
    }
    stats
}

/// Экспорт и повторное измерение результата.
#[derive(Clone, Debug)]
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub after: Measured,
}

pub fn render(song: &Song, limits: &Limits) -> Rendered {
    let bytes = export_smf(song);
    let after = match read_midi(&bytes) {
        Ok(raw) => measure(&raw, limits),
        Err(_) => Measured::default(),
    };
    Rendered { bytes, after }
}

#[derive(Clone, Debug)]
pub struct Processed {
    pub bytes: Vec<u8>,
    pub song: Song,
    pub report: FileReport,
}

pub fn process_bytes(input: &[u8], settings: &Settings) -> Result<Processed, ReadError> {
    Ok(process_loaded(load(input, &settings.limits)?, settings))
}

/// Обработка уже загруженной (возможно, отредактированной) песни.
pub fn process_loaded(loaded: Loaded, settings: &Settings) -> Processed {
    let started = Instant::now();
    let Loaded { mut song, build, before } = loaded;
    let notes_before = song.note_count();
    let stats = optimize(&mut song, settings);
    let rendered = render(&song, &settings.limits);
    let limiter = stats.limiter.unwrap_or_default();
    let share = |n: usize| if notes_before == 0 { 0.0 } else { n as f64 / notes_before as f64 };
    let removed_share = share(limiter.removed.len());
    // «Сильно упрощён» — только по слышимым удалениям: дубли в унисон и микроноты не в счёт.
    let audible_share = share(limiter.removed.iter().filter(|r| r.tier >= Tier::C).count());
    let mut overloaded = problem_ranges(&rendered.after.times_us, &settings.limits);
    overloaded.extend(limiter.unresolved.iter().copied());
    overloaded.sort_by(|a, b| a.from_s.total_cmp(&b.from_s));
    overloaded.dedup_by(|b, a| {
        if b.from_s <= a.to_s {
            a.to_s = a.to_s.max(b.to_s);
            a.peak = a.peak.max(b.peak);
            true
        } else {
            false
        }
    });
    let report = FileReport {
        input: None,
        output: None,
        preset: settings.preset,
        settings: settings.clone(),
        status: status_of(&rendered.after.metrics, &settings.limits),
        recovered: song.source.recovered,
        problems: song.source.problems.clone(),
        before: before.metrics,
        after: rendered.after.metrics.clone(),
        channels: channel_infos(&song),
        changes: Changes {
            build,
            trim: stats.trim,
            velocity: stats.velocity,
            cleanup: stats.cleanup,
            removed_notes: limiter.removed,
            removed_share,
            audible_share,
            heavily_simplified: audible_share > settings.heavy_share,
        },
        overloaded_ranges: overloaded,
        timeline_before: timeline(&before.times_us, TIMELINE_BUCKET_US),
        timeline_after: timeline(&rendered.after.times_us, TIMELINE_BUCKET_US),
        processing_ms: started.elapsed().as_secs_f64() * 1000.0,
    };
    Processed { bytes: rendered.bytes, song, report }
}

pub fn analyze_bytes(input: &[u8], limits: &Limits) -> Result<AnalysisReport, ReadError> {
    let Loaded { song, build, before } = load(input, limits)?;
    Ok(AnalysisReport {
        input: None,
        status: status_of(&before.metrics, limits),
        recovered: song.source.recovered,
        problems: song.source.problems.clone(),
        channels: channel_infos(&song),
        issues: build,
        overloaded_ranges: problem_ranges(&before.times_us, limits),
        timeline: timeline(&before.times_us, TIMELINE_BUCKET_US),
        metrics: before.metrics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Status;
    use crate::testkit::{SmfBuilder, TrackBuilder};

    fn messy() -> Vec<u8> {
        // Пустые note_off, наложения, тишина в начале, громкие ударные и плотный участок.
        // Аккорды из 4 нот каждые 6 тиков (~6 мс) — около 1300 событий в секунду, плюс пустой note_off на аккорд.
        let mut t = TrackBuilder::new().name("Mess").tempo(0, 500_000).off(0, 0, 10).cc(0, 0, 80, 1);
        for i in 0..800u64 {
            let start = 960 + i * 6;
            let root = 40 + (i % 20) as u8;
            for (k, d) in [0u8, 4, 7, 12].into_iter().enumerate() {
                t = t.note(start, start + 40, k as u8, root + d, 60 + 10 * k as u8);
            }
            t = t.off(start + 1, 5, 1);
        }
        t = t.note(960, 1440, 9, 36, 127);
        SmfBuilder::new(480).track(t).bytes()
    }

    #[test]
    fn safe_preset_fixes_everything() {
        let p = process_bytes(&messy(), &Settings::new(Preset::Safe)).unwrap();
        let r = &p.report;
        assert_eq!(r.status, Status::Ok, "{:?}", r.after);
        assert!(r.before.peak_per_second.value > 1000);
        assert!(r.after.peak_per_second.value <= 900);
        assert!(r.changes.build.orphan_offs >= 800);
        assert!(r.changes.trim.trimmed_s > 0.9);
        assert!(!r.changes.removed_notes.is_empty());
        assert!(r.channels.iter().any(|c| c.drums && c.velocity_range == Some((85, 85))));
        assert_eq!(r.changes.cleanup.as_ref().unwrap().unused_removed, 1);
    }

    #[test]
    fn repair_preset_keeps_all_notes() {
        let loaded = load(&messy(), &Limits::default()).unwrap();
        let p = process_bytes(&messy(), &Settings::new(Preset::Repair)).unwrap();
        assert_eq!(p.song.note_count(), loaded.song.note_count());
        assert!(p.report.changes.velocity.is_none());
        // Все начала сдвинуты одинаково.
        let shift = loaded.song.channels[0].notes[0].start as i64 - p.song.channels[0].notes[0].start as i64;
        for (a, b) in loaded.song.channels[0].notes.iter().zip(&p.song.channels[0].notes) {
            assert_eq!(a.start as i64 - b.start as i64, shift);
        }
    }

    #[test]
    fn quality_preset_removes_no_notes() {
        let p = process_bytes(&messy(), &Settings::new(Preset::Quality)).unwrap();
        assert!(p.report.changes.removed_notes.is_empty());
        assert_ne!(p.report.status, Status::Ok);
        assert!(!p.report.overloaded_ranges.is_empty());
    }

    #[test]
    fn file_without_notes_is_fine() {
        let bytes = SmfBuilder::new(96).track(TrackBuilder::new().tempo(0, 500_000)).bytes();
        let p = process_bytes(&bytes, &Settings::default()).unwrap();
        assert_eq!(p.report.status, Status::Ok);
        assert_eq!(p.song.note_count(), 0);
        assert!(read_midi(&p.bytes).is_ok());
    }

    #[test]
    fn analyze_reports_issues() {
        let a = analyze_bytes(&messy(), &Limits::default()).unwrap();
        assert_eq!(a.status, Status::Over);
        assert!(a.issues.orphan_offs >= 800);
        assert!(!a.overloaded_ranges.is_empty());
        assert!(!a.timeline.is_empty());
    }
}
