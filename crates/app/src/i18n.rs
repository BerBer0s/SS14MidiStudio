//! Два языка интерфейса. Строки пишутся прямо в месте использования: `tr("Файлы", "Files")`.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Lang {
    #[default]
    Ru,
    En,
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::En } else { Lang::Ru }
}

pub fn tr(ru: &'static str, en: &'static str) -> &'static str {
    match lang() {
        Lang::Ru => ru,
        Lang::En => en,
    }
}

/// Склонение для русского: 1 файл, 2 файла, 5 файлов.
pub fn count(n: usize, ru: [&str; 3], en: [&str; 2]) -> String {
    match lang() {
        Lang::En => format!("{n} {}", if n == 1 { en[0] } else { en[1] }),
        Lang::Ru => {
            let (m10, m100) = (n % 10, n % 100);
            let w = if m10 == 1 && m100 != 11 {
                ru[0]
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                ru[1]
            } else {
                ru[2]
            };
            format!("{n} {w}")
        }
    }
}

pub fn status_label(s: ss14midi_core::Status) -> &'static str {
    match s {
        ss14midi_core::Status::Ok => tr("Пройдёт", "Fits"),
        ss14midi_core::Status::Warn => tr("На грани", "Borderline"),
        ss14midi_core::Status::Over => tr("Не пройдёт", "Too dense"),
    }
}

pub fn preset_label(p: ss14midi_core::Preset) -> &'static str {
    match p {
        ss14midi_core::Preset::Safe => tr("Безопасный", "Safe"),
        ss14midi_core::Preset::Quality => tr("Без потерь", "Lossless"),
        ss14midi_core::Preset::Repair => tr("Только починка", "Repair only"),
    }
}

pub fn preset_hint(p: ss14midi_core::Preset) -> &'static str {
    match p {
        ss14midi_core::Preset::Safe => tr(
            "Файл гарантированно уложится в лимиты SS14. В перегруженных местах убираются самые незаметные ноты, мелодия и бас не трогаются.",
            "The file is guaranteed to fit SS14 limits. The least noticeable notes are removed where it is overloaded; melody and bass are kept.",
        ),
        ss14midi_core::Preset::Quality => tr(
            "Починка, выравнивание громкости и сжатие без потерь. Ноты не удаляются, поэтому очень плотный файл может остаться перегруженным.",
            "Repair, volume levelling and lossless compaction. No notes are removed, so a very dense file may stay overloaded.",
        ),
        ss14midi_core::Preset::Repair => tr(
            "Только исправление ошибок файла: пустые note_off, наложения, зависшие ноты, тишина в начале. Громкость и плотность не меняются.",
            "Fix file errors only: stray note-offs, overlaps, hanging notes, leading silence. Volume and density stay as they are.",
        ),
    }
}
