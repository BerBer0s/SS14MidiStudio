//! Пакетная обработка: поиск файлов, пути вывода, параллельный запуск, атомарная запись.

use std::collections::HashMap;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use crate::pipeline::{analyze_bytes, process_bytes, Settings};
use crate::read::ReadError;
use crate::report::{AnalysisReport, FileReport};
use crate::Limits;

pub const OUTPUT_SUFFIX: &str = "_ss14";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputFile {
    pub path: PathBuf,
    /// Папка, через которую файл найден (для повторения структуры в `--out`).
    pub root: Option<PathBuf>,
}

pub fn is_midi(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
        .unwrap_or(false)
}

pub fn is_our_output(path: &Path) -> bool {
    path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.ends_with(OUTPUT_SUFFIX))
}

/// Файлы и папки → список MIDI. Возвращает файлы и предупреждения.
pub fn collect_inputs(paths: &[PathBuf]) -> (Vec<InputFile>, Vec<String>) {
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut found = Vec::new();
            walk(p, &mut found);
            found.sort();
            for f in found {
                if !is_our_output(&f) {
                    files.push(InputFile { path: f, root: Some(p.clone()) });
                }
            }
        } else if p.is_file() {
            if !is_midi(p) {
                warnings.push(format!("не MIDI, пропускаю: {}", p.display()));
            } else if is_our_output(p) {
                warnings.push(format!("уже обработан, пропускаю: {}", p.display()));
            } else {
                files.push(InputFile { path: p.clone(), root: None });
            }
        } else {
            warnings.push(format!("не найдено: {}", p.display()));
        }
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(f.path.clone()));
    (files, warnings)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if is_midi(&p) {
            out.push(p);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub input: PathBuf,
    pub output: PathBuf,
    pub warning: Option<String>,
}

pub fn output_name(input: &Path) -> String {
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{stem}{OUTPUT_SUFFIX}.mid")
}

/// Пути вывода. Совпадающие получают суффиксы `_2`, `_3`…
pub fn plan_jobs(inputs: &[InputFile], out_dir: Option<&Path>) -> Vec<Job> {
    let mut used: HashMap<PathBuf, u32> = HashMap::new();
    inputs
        .iter()
        .map(|f| {
            let name = output_name(&f.path);
            let dir = match (out_dir, &f.root) {
                (None, _) => f.path.parent().map(Path::to_path_buf).unwrap_or_default(),
                (Some(out), Some(root)) => {
                    let rel = f.path.parent().and_then(|p| p.strip_prefix(root).ok()).unwrap_or(Path::new(""));
                    out.join(rel)
                }
                (Some(out), None) => out.to_path_buf(),
            };
            let mut output = dir.join(&name);
            let mut warning = None;
            let key = normalize(&output);
            let n = used.entry(key).or_insert(0);
            *n += 1;
            if *n > 1 {
                let stem = name.trim_end_matches(".mid");
                output = dir.join(format!("{stem}_{n}.mid"));
                warning = Some(format!("имя совпало с другим файлом — сохраняю как {}", output.display()));
                used.insert(normalize(&output), 1);
            }
            Job { input: f.path.clone(), output, warning }
        })
        .collect()
}

fn normalize(p: &Path) -> PathBuf {
    PathBuf::from(p.to_string_lossy().to_lowercase())
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq, Serialize)]
pub enum JobError {
    #[error("{0}")]
    Read(String),
    #[error("не удалось прочитать файл: {0}")]
    Io(String),
    #[error("не удалось записать результат: {0}")]
    WriteFailed(String),
    #[error("путь результата совпадает с исходником")]
    OutputIsInput,
    #[error("внутренняя ошибка (это баг): {0}")]
    Internal(String),
}

impl From<ReadError> for JobError {
    fn from(e: ReadError) -> JobError {
        JobError::Read(e.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct JobResult {
    pub job: Job,
    pub outcome: Result<FileReport, JobError>,
}

/// Запись через временный файл и переименование: сбой не оставит половину файла.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "паника".into())
}

pub fn run_job(job: &Job, settings: &Settings) -> Result<FileReport, JobError> {
    if normalize(&job.input) == normalize(&job.output) {
        return Err(JobError::OutputIsInput);
    }
    let bytes = std::fs::read(&job.input).map_err(|e| JobError::Io(e.to_string()))?;
    let processed = catch_unwind(AssertUnwindSafe(|| process_bytes(&bytes, settings)))
        .map_err(|e| JobError::Internal(panic_text(e)))??;
    write_atomic(&job.output, &processed.bytes).map_err(|e| JobError::WriteFailed(e.to_string()))?;
    let mut report = processed.report;
    report.input = Some(job.input.display().to_string());
    report.output = Some(job.output.display().to_string());
    Ok(report)
}

/// Параллельная обработка. `on_done` вызывается по мере готовности (из рабочих потоков).
pub fn run_jobs(jobs: &[Job], settings: &Settings, threads: Option<usize>, on_done: impl Fn(&JobResult) + Sync) -> Vec<JobResult> {
    let work = || {
        jobs.par_iter()
            .map(|job| {
                let result = JobResult { job: job.clone(), outcome: run_job(job, settings) };
                on_done(&result);
                result
            })
            .collect()
    };
    match threads.and_then(|n| rayon::ThreadPoolBuilder::new().num_threads(n).build().ok()) {
        Some(pool) => pool.install(work),
        None => work(),
    }
}

pub fn analyze_file(path: &Path, limits: &Limits) -> Result<AnalysisReport, JobError> {
    let bytes = std::fs::read(path).map_err(|e| JobError::Io(e.to_string()))?;
    let mut report = catch_unwind(AssertUnwindSafe(|| analyze_bytes(&bytes, limits)))
        .map_err(|e| JobError::Internal(panic_text(e)))??;
    report.input = Some(path.display().to_string());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{SmfBuilder, TrackBuilder};

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ss14midi-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn midi() -> Vec<u8> {
        SmfBuilder::new(96).track(TrackBuilder::new().note(0, 96, 0, 60, 100)).bytes()
    }

    #[test]
    fn collects_recursively_and_skips_outputs() {
        let d = tmp_dir("collect");
        std::fs::create_dir_all(d.join("Папка с пробелом")).unwrap();
        std::fs::write(d.join("a.MID"), midi()).unwrap();
        std::fs::write(d.join("Папка с пробелом").join("б.midi"), midi()).unwrap();
        std::fs::write(d.join("a_ss14.mid"), midi()).unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();
        let (files, warnings) = collect_inputs(&[d.clone(), d.join("a.MID")]);
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(warnings.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn output_paths_mirror_structure_and_dedupe() {
        let root = PathBuf::from("C:/in");
        let inputs = vec![
            InputFile { path: root.join("x/song.mid"), root: Some(root.clone()) },
            InputFile { path: PathBuf::from("D:/other/song.mid"), root: None },
            InputFile { path: PathBuf::from("E:/third/song.mid"), root: None },
        ];
        let jobs = plan_jobs(&inputs, Some(Path::new("C:/out")));
        assert_eq!(jobs[0].output, Path::new("C:/out").join("x").join("song_ss14.mid"));
        assert_eq!(jobs[1].output, Path::new("C:/out").join("song_ss14.mid"));
        assert_eq!(jobs[2].output, Path::new("C:/out").join("song_ss14_2.mid"));
        assert!(jobs[2].warning.is_some());
        let near = plan_jobs(&inputs[1..2], None);
        assert_eq!(near[0].output, Path::new("D:/other").join("song_ss14.mid"));
    }

    #[test]
    fn runs_jobs_and_reports_errors() {
        let d = tmp_dir("run");
        std::fs::write(d.join("good.mid"), midi()).unwrap();
        std::fs::write(d.join("bad.mid"), b"garbage").unwrap();
        let (files, _) = collect_inputs(std::slice::from_ref(&d));
        let jobs = plan_jobs(&files, None);
        let results = run_jobs(&jobs, &Settings::default(), Some(2), |_| {});
        let ok = results.iter().filter(|r| r.outcome.is_ok()).count();
        assert_eq!(ok, 1);
        assert!(d.join("good_ss14.mid").exists());
        assert!(!d.join("good_ss14.mid.tmp").exists());
        let bad = results.iter().find(|r| r.outcome.is_err()).unwrap();
        assert_eq!(bad.outcome.as_ref().unwrap_err(), &JobError::Read("это не MIDI-файл".into()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
