//! Внутренняя модель песни: ноты как пары «начало–конец», контроллеры, карта темпа.

use serde::Serialize;

/// Индекс канала ударных (канал 10 в нумерации с единицы).
pub const DRUM_CHANNEL: usize = 9;
pub const CHANNELS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Division {
    /// Тиков на четверть.
    Ticks(u16),
    /// SMPTE: кадров в секунду (24, 25, 29, 30) и тиков на кадр.
    Smpte { fps: u8, ticks_per_frame: u8 },
}

impl Division {
    /// Сколько тиков считать «четвертью» там, где нужна длина по умолчанию.
    pub fn quarter_ticks(self) -> u64 {
        match self {
            Division::Ticks(t) => t.max(1) as u64,
            // Для SMPTE — полсекунды.
            Division::Smpte { fps, ticks_per_frame } => (fps as u64 * ticks_per_frame as u64 / 2).max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub start: u64,
    pub end: u64,
    pub key: u8,
    pub velocity: u8,
}

impl Note {
    pub fn len(&self) -> u64 {
        self.end - self.start
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum ControlKind {
    Program,
    Cc(u8),
    PitchBend,
    ChannelPressure,
    PolyPressure(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Control {
    pub tick: u64,
    pub kind: ControlKind,
    /// 0..=127, для pitch bend 0..=16383 (8192 — центр).
    pub value: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Channel {
    pub name: Option<String>,
    pub notes: Vec<Note>,
    pub controls: Vec<Control>,
}

impl Channel {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.controls.is_empty()
    }

    /// Первая смена инструмента в канале.
    pub fn first_program(&self) -> Option<u8> {
        self.controls
            .iter()
            .find(|c| c.kind == ControlKind::Program)
            .map(|c| c.value as u8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaKind {
    /// Микросекунд на четверть.
    Tempo(u32),
    TimeSignature([u8; 4]),
    KeySignature(i8, bool),
}

impl MetaKind {
    pub fn type_order(&self) -> u8 {
        match self {
            MetaKind::Tempo(_) => 0,
            MetaKind::TimeSignature(_) => 1,
            MetaKind::KeySignature(..) => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetaEvent {
    pub tick: u64,
    pub kind: MetaKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysEx {
    pub tick: u64,
    /// Полезная нагрузка после байта 0xF0 (включая завершающий 0xF7, если был).
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SourceInfo {
    pub format: u16,
    pub tracks: usize,
    pub recovered: bool,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    pub division: Division,
    /// Темп, размер, тональность — отсортированы по тику.
    pub meta: Vec<MetaEvent>,
    pub channels: Vec<Channel>,
    pub sysex: Vec<SysEx>,
    pub source: SourceInfo,
}

impl Song {
    pub fn empty(division: Division) -> Song {
        Song {
            division,
            meta: Vec::new(),
            channels: vec![Channel::default(); CHANNELS],
            sysex: Vec::new(),
            source: SourceInfo::default(),
        }
    }

    pub fn tempo_map(&self) -> crate::timing::TempoMap {
        crate::timing::TempoMap::from_meta(self.division, &self.meta)
    }

    pub fn note_count(&self) -> usize {
        self.channels.iter().map(|c| c.notes.len()).sum()
    }

    /// Последний тик, на котором что-то происходит.
    pub fn end_tick(&self) -> u64 {
        let notes = self.channels.iter().flat_map(|c| c.notes.iter().map(|n| n.end));
        let controls = self.channels.iter().flat_map(|c| c.controls.iter().map(|c| c.tick));
        let meta = self.meta.iter().map(|m| m.tick);
        let sysex = self.sysex.iter().map(|s| s.tick);
        notes.chain(controls).chain(meta).chain(sysex).max().unwrap_or(0)
    }

    /// Отличное от «по умолчанию» имя канала для вывода и интерфейса.
    pub fn channel_label(&self, ch: usize) -> String {
        if let Some(name) = &self.channels[ch].name {
            return name.clone();
        }
        if ch == DRUM_CHANNEL {
            return "Drums".to_string();
        }
        match self.channels[ch].first_program() {
            Some(p) => crate::gm::gm_name(p).to_string(),
            None => format!("Channel {}", ch + 1),
        }
    }
}
