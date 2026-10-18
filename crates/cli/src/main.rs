//! ss14midi — консольная версия SS14 MIDI Studio.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Instant;

use clap::{Args, Parser, Subcommand, ValueEnum};
use ss14midi_core::batch::{analyze_file, collect_inputs, plan_jobs, run_jobs, JobResult};
use ss14midi_core::passes::velocity::VelocityMode;
use ss14midi_core::report::{AnalysisReport, FileReport};
use ss14midi_core::{Limits, Preset, Settings, Status};

#[derive(Parser)]
#[command(name = "ss14midi", version, about = "Оптимизатор MIDI для инструментов Space Station 14")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Проверить файлы, ничего не записывая.
    Analyze {
        /// Файлы .mid/.midi или папки.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Отчёт в JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        limits: LimitArgs,
    },
    /// Обработать файлы.
    Fix(FixArgs),
}

#[derive(Clone, Copy, ValueEnum)]
enum PresetArg {
    Safe,
    Quality,
    Repair,
}

#[derive(Args)]
struct LimitArgs {
    /// Цель: событий в любой секунде.
    #[arg(long, default_value_t = 900)]
    target_per_second: u32,
    /// Жёсткий лимит сервера: событий в секунду.
    #[arg(long, default_value_t = 1000)]
    max_per_second: u32,
    /// Лимит событий за кадр.
    #[arg(long, default_value_t = 60)]
    max_per_frame: u32,
}

impl LimitArgs {
    fn limits(&self) -> Limits {
        Limits {
            max_per_second: self.max_per_second,
            target_per_second: self.target_per_second.min(self.max_per_second),
            frame_limit: self.max_per_frame,
            ..Limits::default()
        }
    }
}

#[derive(Args)]
struct FixArgs {
    /// Файлы .mid/.midi или папки.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// safe — гарантировать лимиты, quality — без удаления нот, repair — только починка.
    #[arg(long, value_enum, default_value = "safe")]
    preset: PresetArg,
    /// Папка для результатов (по умолчанию — рядом с исходниками).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Отчёт в JSON.
    #[arg(long)]
    json: bool,
    /// Громкость ударных (velocity).
    #[arg(long)]
    drum_cap: Option<u8>,
    /// Громкость мелодии (velocity).
    #[arg(long)]
    melodic_cap: Option<u8>,
    /// Нормализовать громкость каждого канала отдельно.
    #[arg(long)]
    per_channel_velocity: bool,
    /// Приглушать плотные аккорды.
    #[arg(long)]
    chord_softening: bool,
    /// Число потоков.
    #[arg(long)]
    jobs: Option<usize>,
    #[command(flatten)]
    limits: LimitArgs,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // Перетаскивание файлов на exe: аргументы — только пути.
    let drag = args.len() > 1
        && !matches!(args[1].as_str(), "analyze" | "fix" | "help" | "-h" | "--help" | "-V" | "--version")
        && args[1..].iter().all(|a| !a.starts_with('-'));
    if drag {
        let paths = args[1..].iter().map(PathBuf::from).collect();
        let code = fix(FixArgs {
            paths,
            preset: PresetArg::Safe,
            out: None,
            json: false,
            drum_cap: None,
            melodic_cap: None,
            per_channel_velocity: false,
            chord_softening: false,
            jobs: None,
            limits: LimitArgs { target_per_second: 900, max_per_second: 1000, max_per_frame: 60 },
        });
        print!("\nНажмите Enter, чтобы закрыть…");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().lock().read_line(&mut String::new());
        return code;
    }
    match Cli::parse().cmd {
        Cmd::Analyze { paths, json, limits } => analyze(paths, json, limits.limits()),
        Cmd::Fix(a) => fix(a),
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

fn status_tag(s: Status) -> &'static str {
    match s {
        Status::Ok => "ОК        ",
        Status::Warn => "НА ГРАНИ  ",
        Status::Over => "ПРЕВЫШЕНИЕ",
    }
}

fn plural(n: usize, one: &str, few: &str, many: &str) -> String {
    let (m10, m100) = (n % 10, n % 100);
    let w = if m10 == 1 && m100 != 11 {
        one
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        few
    } else {
        many
    };
    format!("{n} {w}")
}

fn print_fix(r: &FileReport) {
    let input = r.input.as_deref().map(file_name).unwrap_or_default();
    let output = r.output.as_deref().map(file_name).unwrap_or_default();
    println!("{} {input} → {output}", status_tag(r.status));
    let b = &r.changes.build;
    let mut parts = vec![format!(
        "пик {}/с (было {}) · кадр {} (было {})",
        r.after.peak_per_second.value, r.before.peak_per_second.value, r.after.peak_per_frame.value, r.before.peak_per_frame.value
    )];
    let fixed: Vec<String> = [
        (b.orphan_offs, "пустых note_off"),
        (b.overlaps_resolved + b.same_start_merged, "наложений"),
        (b.hanging_closed, "зависших"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, w)| format!("{w} {n}"))
    .collect();
    if !fixed.is_empty() {
        parts.push(format!("починено: {}", fixed.join(", ")));
    }
    if !r.changes.removed_notes.is_empty() {
        parts.push(format!("удалено нот {} ({:.1}%)", r.changes.removed_notes.len(), r.changes.removed_share * 100.0));
    }
    println!("           {}", parts.join(" · "));
    if r.changes.heavily_simplified {
        println!("           ⚠ сильно упрощён: стоит разбить песню на партии для нескольких игроков");
    }
    if r.recovered {
        println!("           файл был повреждён и восстановлен: {}", r.problems.join("; "));
    }
    if r.status != Status::Ok {
        let ranges: Vec<String> = r.overloaded_ranges.iter().take(5).map(|x| format!("{:.1}–{:.1} с", x.from_s, x.to_s)).collect();
        println!("           перегрузка: {}", ranges.join(", "));
    }
}

fn fix(a: FixArgs) -> ExitCode {
    let started = Instant::now();
    let preset = match a.preset {
        PresetArg::Safe => Preset::Safe,
        PresetArg::Quality => Preset::Quality,
        PresetArg::Repair => Preset::Repair,
    };
    let mut settings = Settings::new(preset);
    settings.limits = a.limits.limits();
    let velocity_flags = a.drum_cap.is_some() || a.melodic_cap.is_some() || a.per_channel_velocity || a.chord_softening;
    if velocity_flags && !settings.velocity_enabled() {
        eprintln!("Флаги громкости игнорируются: пресет «{}» не меняет громкость.", preset.label_ru());
    }
    if let Some(v) = a.drum_cap {
        settings.velocity.drum_cap = v.clamp(1, 127);
    }
    if let Some(v) = a.melodic_cap {
        settings.velocity.melodic_cap = v.clamp(1, 127);
    }
    if a.per_channel_velocity {
        settings.velocity.mode = VelocityMode::PerChannel;
    }
    settings.velocity.chord_softening = a.chord_softening;

    let (files, warnings) = collect_inputs(&a.paths);
    for w in &warnings {
        eprintln!("{w}");
    }
    if files.is_empty() {
        eprintln!("MIDI-файлы не найдены.");
        return ExitCode::from(1);
    }
    let jobs = plan_jobs(&files, a.out.as_deref());
    for j in &jobs {
        if let Some(w) = &j.warning {
            eprintln!("{w}");
        }
    }
    if !a.json {
        println!("Обрабатываю {} · пресет «{}»\n", plural(files.len(), "файл", "файла", "файлов"), preset.label_ru());
    }
    let lock = Mutex::new(());
    let json = a.json;
    let results = run_jobs(&jobs, &settings, a.jobs, |r: &JobResult| {
        if json {
            return;
        }
        let _g = lock.lock().unwrap();
        match &r.outcome {
            Ok(report) => print_fix(report),
            Err(e) => println!("ОШИБКА     {} — {e}", r.job.input.display()),
        }
    });

    let (mut ok, mut warn, mut over, mut err) = (0, 0, 0, 0);
    for r in &results {
        match &r.outcome {
            Ok(rep) => match rep.status {
                Status::Ok => ok += 1,
                Status::Warn => warn += 1,
                Status::Over => over += 1,
            },
            Err(_) => err += 1,
        }
    }
    if json {
        let list: Vec<serde_json::Value> = results
            .iter()
            .map(|r| match &r.outcome {
                Ok(rep) => serde_json::to_value(rep).unwrap_or_default(),
                Err(e) => serde_json::json!({ "input": r.job.input.display().to_string(), "error": e.to_string() }),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
    } else {
        println!(
            "\nИтого: {} — ОК {ok} · на грани {warn} · превышение {over} · ошибок {err} · {:.2} с",
            plural(results.len(), "файл", "файла", "файлов"),
            started.elapsed().as_secs_f64()
        );
    }
    exit_code(err, over)
}

fn exit_code(errors: usize, over: usize) -> ExitCode {
    if errors > 0 {
        ExitCode::from(1)
    } else if over > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_analysis(r: &AnalysisReport) {
    let name = r.input.as_deref().map(file_name).unwrap_or_default();
    let m = &r.metrics;
    println!("{} {name}", status_tag(r.status));
    println!(
        "           {:.0} с · событий {} · пик {}/с · кадр {} · каналов {}",
        m.duration_s, m.events_total, m.peak_per_second.value, m.peak_per_frame.value, m.channels_used
    );
    let i = &r.issues;
    let issues: Vec<String> = [
        (i.orphan_offs, "пустых note_off"),
        (i.overlaps_resolved + i.same_start_merged, "наложений"),
        (i.hanging_closed, "зависших нот"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, w)| format!("{w} {n}"))
    .collect();
    if !issues.is_empty() {
        println!("           проблемы: {}", issues.join(", "));
    }
    if r.recovered {
        println!("           повреждения: {}", r.problems.join("; "));
    }
}

fn analyze(paths: Vec<PathBuf>, json: bool, limits: Limits) -> ExitCode {
    let (files, warnings) = collect_inputs(&paths);
    for w in &warnings {
        eprintln!("{w}");
    }
    let results: Vec<_> = files.iter().map(|f| (f, analyze_file(&f.path, &limits))).collect();
    let (mut over, mut err) = (0, 0);
    for (_, r) in &results {
        match r {
            Ok(a) if a.status == Status::Over => over += 1,
            Err(_) => err += 1,
            _ => {}
        }
    }
    if json {
        let list: Vec<serde_json::Value> = results
            .iter()
            .map(|(f, r)| match r {
                Ok(a) => serde_json::to_value(a).unwrap_or_default(),
                Err(e) => serde_json::json!({ "input": f.path.display().to_string(), "error": e.to_string() }),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
    } else {
        for (f, r) in &results {
            match r {
                Ok(a) => print_analysis(a),
                Err(e) => println!("ОШИБКА     {} — {e}", f.path.display()),
            }
        }
        if std::io::stdout().is_terminal() {
            println!("\nПроверено: {} · превышают лимиты: {over} · ошибок: {err}", plural(results.len(), "файл", "файла", "файлов"));
        }
    }
    exit_code(err, over)
}
