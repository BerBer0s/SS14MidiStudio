//! Прогон на реальных файлах. Папки задаются переменной `SS14MIDI_CORPUS` (через `;`).
//! Без переменной тест ничего не делает.
//!
//! cargo test --release -p ss14midi-core --test corpus -- --nocapture

use std::path::PathBuf;
use std::time::Instant;

use ss14midi_core::build::build_song;
use ss14midi_core::pipeline::{load, process_bytes, Preset, Settings};
use ss14midi_core::read::{read_midi, RawKind};
use ss14midi_core::Status;

fn corpus() -> Vec<PathBuf> {
    let Ok(dirs) = std::env::var("SS14MIDI_CORPUS") else { return Vec::new() };
    let mut files = Vec::new();
    for dir in dirs.split(';').filter(|d| !d.trim().is_empty()) {
        let Ok(entries) = std::fs::read_dir(dir.trim()) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext == "mid" || ext == "midi" {
                files.push(p);
            }
        }
    }
    files.sort();
    files
}

/// В результате нет пустых note_off, зависших нот и наложений на одной клавише.
fn assert_clean(bytes: &[u8], name: &str) {
    let raw = read_midi(bytes).unwrap();
    assert!(raw.problems.is_empty(), "{name}: результат читается с проблемами {:?}", raw.problems);
    let (_, stats) = build_song(&raw);
    assert_eq!(stats.orphan_offs, 0, "{name}: пустые note_off");
    assert_eq!(stats.hanging_closed, 0, "{name}: зависшие ноты");
    assert_eq!(stats.overlaps_resolved + stats.same_start_merged, 0, "{name}: наложения");
    // На одном тике note_off идёт раньше note_on.
    for t in &raw.tracks {
        for w in t.events.windows(2) {
            if w[0].tick == w[1].tick {
                assert!(
                    !(matches!(w[0].kind, RawKind::NoteOn { .. }) && matches!(w[1].kind, RawKind::NoteOff { .. })),
                    "{name}: note_on раньше note_off на одном тике"
                );
            }
        }
    }
}

#[test]
fn corpus_all_presets() {
    let files = corpus();
    if files.is_empty() {
        eprintln!("SS14MIDI_CORPUS не задан — пропуск");
        return;
    }
    println!(
        "{:<52} {:>9} {:>9} {:>7} {:>7}  {:>6} {:>7}  статус",
        "файл", "было/с", "стало/с", "кадр", "→кадр", "удал.%", "мс"
    );
    let started = Instant::now();
    let mut not_ok = Vec::new();
    for path in &files {
        let name: String = path.file_name().unwrap().to_string_lossy().chars().take(50).collect();
        let bytes = std::fs::read(path).unwrap();
        for preset in Preset::ALL {
            let p = process_bytes(&bytes, &Settings::new(preset)).unwrap();
            assert_clean(&p.bytes, &name);
            let r = &p.report;
            if preset == Preset::Repair {
                let loaded = load(&bytes, &Settings::default().limits).unwrap();
                assert_eq!(p.song.note_count(), loaded.song.note_count(), "{name}: починка потеряла ноты");
            }
            if preset == Preset::Safe {
                println!(
                    "{:<52} {:>9} {:>9} {:>7} {:>7}  {:>6.1} {:>7.1}  {:?}{}",
                    name,
                    r.before.peak_per_second.value,
                    r.after.peak_per_second.value,
                    r.before.peak_per_frame.value,
                    r.after.peak_per_frame.value,
                    r.changes.removed_share * 100.0,
                    r.processing_ms,
                    r.status,
                    if r.changes.heavily_simplified { " (сильно упрощён)" } else { "" }
                );
                if r.status != Status::Ok {
                    not_ok.push((name.clone(), r.overloaded_ranges.clone()));
                }
            }
        }
    }
    println!("\n{} файлов × 3 пресета за {:.2} с", files.len(), started.elapsed().as_secs_f64());
    for (name, ranges) in &not_ok {
        println!("НЕ ОК: {name}: {ranges:?}");
    }
}
