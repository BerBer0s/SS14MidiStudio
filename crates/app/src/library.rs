//! Открытые файлы, их рабочие копии, история правок и фоновые задачи.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use ss14midi_core::analysis::{problem_ranges, status_of, timeline, Metrics, TimeRange, TIMELINE_BUCKET_US};
use ss14midi_core::batch::{output_name, write_atomic};
use ss14midi_core::build::BuildStats;
use ss14midi_core::passes::limiter::RemovedNote;
use ss14midi_core::pipeline::{load, process_loaded, render, Loaded};
use ss14midi_core::read::read_midi;
use ss14midi_core::report::FileReport;
use ss14midi_core::{Limits, Settings, Song, Status};

/// Как файл прозвучит в SS14 — по экспортированным байтам.
#[derive(Clone, Debug)]
pub struct Analysis {
    pub status: Status,
    pub metrics: Metrics,
    pub timeline: Vec<u32>,
    pub ranges: Vec<TimeRange>,
}

impl Analysis {
    fn of_song(song: &Song, limits: &Limits) -> Analysis {
        let rendered = render(song, limits);
        Analysis {
            status: status_of(&rendered.after.metrics, limits),
            ranges: problem_ranges(&rendered.after.times_us, limits),
            timeline: timeline(&rendered.after.times_us, TIMELINE_BUCKET_US),
            metrics: rendered.after.metrics,
        }
    }
}

/// Исходный файл как есть.
#[derive(Clone, Debug)]
pub struct Source {
    pub analysis: Analysis,
    pub issues: BuildStats,
    pub problems: Vec<String>,
}

const HISTORY_DEPTH: usize = 40;

#[derive(Default)]
pub struct History {
    undo: Vec<(String, Song)>,
    redo: Vec<(String, Song)>,
}

impl History {
    pub fn push(&mut self, label: impl Into<String>, before: Song) {
        self.undo.push((label.into(), before));
        if self.undo.len() > HISTORY_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &mut Song) -> Option<String> {
        let (label, song) = self.undo.pop()?;
        let now = std::mem::replace(current, song);
        self.redo.push((label.clone(), now));
        Some(label)
    }

    pub fn redo(&mut self, current: &mut Song) -> Option<String> {
        let (label, song) = self.redo.pop()?;
        let now = std::mem::replace(current, song);
        self.undo.push((label.clone(), now));
        Some(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

pub enum EntryState {
    Loading,
    Ready,
    Failed(String),
}

pub struct Entry {
    pub id: u64,
    pub path: PathBuf,
    pub root: Option<PathBuf>,
    pub state: EntryState,
    pub original: Option<Arc<Song>>,
    pub source: Option<Source>,
    pub working: Song,
    pub history: History,
    /// Номер версии рабочей копии; анализ догоняет его в фоне.
    pub revision: u64,
    pub analysis: Option<Analysis>,
    pub analyzed_revision: u64,
    pub report: Option<FileReport>,
    pub ghosts: Vec<RemovedNote>,
    pub busy: bool,
    pub dirty: bool,
    pub saved_to: Option<PathBuf>,
}

impl Entry {
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.state, EntryState::Ready)
    }

    /// Текущий статус: по свежему анализу рабочей копии, иначе по исходнику.
    pub fn status(&self) -> Option<Status> {
        self.analysis.as_ref().map(|a| a.status).or(self.source.as_ref().map(|s| s.analysis.status))
    }

    pub fn analysis_stale(&self) -> bool {
        self.analyzed_revision != self.revision
    }

    /// Зафиксировать правку: старое состояние уходит в историю.
    pub fn commit(&mut self, label: impl Into<String>, before: Song) {
        self.history.push(label, before);
        self.touch();
    }

    pub fn touch(&mut self) {
        self.revision += 1;
        self.dirty = true;
    }
}

pub enum Msg {
    Loaded { id: u64, result: Result<Box<(Loaded, Analysis)>, String> },
    Analyzed { id: u64, revision: u64, analysis: Analysis },
    Processed { id: u64, revision: u64, song: Song, report: Box<FileReport> },
    Saved { id: u64, path: PathBuf, result: Result<(), String> },
}

pub fn output_path(entry: &Entry, out_dir: Option<&Path>) -> PathBuf {
    let name = output_name(&entry.path);
    match (out_dir, &entry.root) {
        (None, _) => entry.path.parent().map(Path::to_path_buf).unwrap_or_default().join(name),
        (Some(out), Some(root)) => {
            let rel = entry.path.parent().and_then(|p| p.strip_prefix(root).ok()).unwrap_or(Path::new(""));
            out.join(rel).join(name)
        }
        (Some(out), None) => out.join(name),
    }
}

pub struct Jobs {
    pub tx: Sender<Msg>,
    pub ctx: egui::Context,
}

impl Jobs {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }

    pub fn load(&self, id: u64, path: PathBuf, limits: Limits) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        rayon::spawn(move || {
            let result = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| load(&bytes, &limits).map_err(|e| e.to_string()))
                .map(|loaded| {
                    let status = status_of(&loaded.before.metrics, &limits);
                    let a = Analysis {
                        status,
                        ranges: problem_ranges(&loaded.before.times_us, &limits),
                        timeline: timeline(&loaded.before.times_us, TIMELINE_BUCKET_US),
                        metrics: loaded.before.metrics.clone(),
                    };
                    Box::new((loaded, a))
                });
            let _ = tx.send(Msg::Loaded { id, result });
            ctx.request_repaint();
        });
    }

    pub fn analyze(&self, id: u64, revision: u64, song: Song, limits: Limits) {
        let jobs = Jobs { tx: self.tx.clone(), ctx: self.ctx.clone() };
        rayon::spawn(move || {
            let analysis = Analysis::of_song(&song, &limits);
            jobs.send(Msg::Analyzed { id, revision, analysis });
        });
    }

    pub fn process(&self, id: u64, revision: u64, song: Song, settings: Settings) {
        let jobs = Jobs { tx: self.tx.clone(), ctx: self.ctx.clone() };
        rayon::spawn(move || {
            let before = render(&song, &settings.limits).after;
            let loaded = Loaded { song, build: BuildStats::default(), before };
            let p = process_loaded(loaded, &settings);
            jobs.send(Msg::Processed { id, revision, song: p.song, report: Box::new(p.report) });
        });
    }

    pub fn save(&self, id: u64, song: Song, path: PathBuf) {
        let jobs = Jobs { tx: self.tx.clone(), ctx: self.ctx.clone() };
        rayon::spawn(move || {
            let bytes = ss14midi_core::export::export_smf(&song);
            let result = match read_midi(&bytes) {
                Ok(_) => write_atomic(&path, &bytes).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            jobs.send(Msg::Saved { id, path, result });
        });
    }
}
