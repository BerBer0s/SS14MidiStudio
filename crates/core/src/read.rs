//! Чтение SMF в «сырые» дорожки с абсолютными тиками. Терпимо к повреждениям:
//! всё, что удалось прочитать, сохраняется, а каждая проблема записывается.

use std::collections::BTreeSet;

use crate::model::{ControlKind, Division, MetaKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawKind {
    NoteOn { ch: u8, key: u8, vel: u8 },
    NoteOff { ch: u8, key: u8 },
    Control { ch: u8, kind: ControlKind, value: u16 },
    SysEx(Vec<u8>),
    Meta(MetaKind),
    TrackName(Vec<u8>),
    EndOfTrack,
    OtherMeta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawEvent {
    pub tick: u64,
    pub kind: RawKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawTrack {
    pub events: Vec<RawEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawSong {
    pub format: u16,
    pub division: Division,
    pub tracks: Vec<RawTrack>,
    pub problems: Vec<String>,
}

impl RawSong {
    pub fn recovered(&self) -> bool {
        !self.problems.is_empty()
    }

    /// Максимальный тик любого события файла.
    pub fn end_tick(&self) -> u64 {
        self.tracks.iter().filter_map(|t| t.events.last().map(|e| e.tick)).max().unwrap_or(0)
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ReadError {
    #[error("это не MIDI-файл")]
    NotMidi,
    #[error("не удалось прочитать ни одной дорожки")]
    NothingRecovered,
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Снять обёртку RIFF RMID, если она есть.
fn unwrap_riff(bytes: &[u8]) -> &[u8] {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"RMID" {
        return bytes;
    }
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let start = pos + 8;
        if id == b"data" {
            return &bytes[start..(start + len).min(bytes.len())];
        }
        pos = start + len + (len & 1);
    }
    bytes
}

pub fn read_midi(bytes: &[u8]) -> Result<RawSong, ReadError> {
    let bytes = unwrap_riff(bytes);
    if bytes.len() < 14 || &bytes[0..4] != b"MThd" {
        return Err(ReadError::NotMidi);
    }
    let mut problems = Vec::new();
    let header_len = be32(&bytes[4..8]) as usize;
    let mut format = be16(&bytes[8..10]);
    let raw_div = be16(&bytes[12..14]);
    if format > 2 {
        problems.push(format!("битый заголовок: формат {format} (допустимо 0, 1, 2) — считаю 1"));
        format = 1;
    }
    let division = if raw_div & 0x8000 != 0 {
        let fps = (-((raw_div >> 8) as u8 as i8)) as u8;
        let fps = if [24, 25, 29, 30].contains(&fps) { fps } else { 30 };
        Division::Smpte { fps, ticks_per_frame: (raw_div & 0xFF).max(1) as u8 }
    } else if raw_div == 0 {
        problems.push("в заголовке 0 тиков на четверть — считаю 480".into());
        Division::Ticks(480)
    } else {
        Division::Ticks(raw_div)
    };

    let mut pos = 8 + header_len.max(6);
    if header_len != 6 {
        problems.push(format!("необычная длина заголовка: {header_len}"));
    }
    let mut tracks = Vec::new();
    let mut skipped_garbage = 0usize;
    let mut overflowed = 0usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        if id != b"MTrk" {
            if id.iter().all(|b| b.is_ascii_alphanumeric()) && pos + 8 <= bytes.len() {
                // Неизвестный, но корректно оформленный чанк — пропускаем целиком.
                let len = be32(&bytes[pos + 4..pos + 8]) as usize;
                if &bytes[pos..pos + 4] != b"MThd" && pos + 8 + len <= bytes.len() {
                    pos += 8 + len;
                    continue;
                }
            }
            pos += 1;
            skipped_garbage += 1;
            continue;
        }
        let len = be32(&bytes[pos + 4..pos + 8]) as usize;
        let start = pos + 8;
        let mut end = start + len;
        let index = tracks.len();
        if end > bytes.len() {
            problems.push(format!("дорожка {index}: заявленная длина выходит за конец файла — читаю до конца"));
            end = bytes.len();
        }
        let data = &bytes[start..end];
        let (mut track, mut problem) = parse_track(data);
        if problem.is_some() || !has_audible_notes(&track) {
            // Чистый стандартный разбор уступает, только если он «разъехался»: байты статусов
            // прочитаны как время, и события ушли дальше, чем при чтении с переполнением.
            let end_of = |t: &RawTrack| t.events.last().map_or(0, |e| e.tick);
            if let Some(t) = parse_overflowed(data).filter(|t| problem.is_some() || end_of(t) < end_of(&track)) {
                track = t;
                problem = None;
                overflowed += 1;
            }
        }
        if let Some(p) = problem {
            problems.push(format!("дорожка {index}: {p}"));
        }
        tracks.push(track);
        pos = end;
    }
    if skipped_garbage > 0 {
        problems.push(format!("пропущено {skipped_garbage} байт мусора между дорожками"));
    }
    if tracks.is_empty() {
        return Err(ReadError::NothingRecovered);
    }
    if overflowed > 0 {
        problems.push(format!("дорожек с номером канала больше 16: {overflowed} — их ноты перенесены на каналы 1–16"));
    }
    let silent = silence_ghost_tracks(&mut tracks);
    if silent > 0 {
        problems.push(format!("дорожек только с беззвучными нотами: {silent} — их контроллеры и смены инструмента не применяются"));
    }
    // Расхождение заявленного числа дорожек с фактическим — частая и безобидная ошибка экспортёров,
    // повреждением не считаем.
    Ok(RawSong { format, division, tracks, problems })
}

fn read_vlq(data: &[u8], pos: &mut usize) -> Option<u32> {
    let mut value: u32 = 0;
    for _ in 0..4 {
        let b = *data.get(*pos)?;
        *pos += 1;
        value = (value << 7) | (b & 0x7F) as u32;
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    pos: usize,
    tick: u64,
    running: Option<u8>,
    /// Номер события в дорожке.
    index: usize,
    /// Сколько событий уже выдано.
    emitted: usize,
    /// Сколько пропущено системных сообщений 0xF1–0xF3, недопустимых в файле.
    system_skipped: usize,
}

#[derive(Clone, Copy, Debug)]
struct Checkpoint {
    at: Cursor,
    /// Канальное сообщение с двумя байтами данных — кандидат на «недостающий байт».
    two_byte: bool,
}

struct Pass {
    events: Vec<RawEvent>,
    checkpoints: Vec<Checkpoint>,
    error: Option<String>,
    /// Где остановились (при ошибке — начало сбойного события).
    cur: Cursor,
    finished: bool,
}

/// Разбор одной дорожки. Некоторые экспортёры пишут канальное сообщение с одним байтом данных
/// вместо двух — после этого всё «съезжает». Если дорожка не читается, пробуем считать одно из
/// предыдущих сообщений укороченным и берём вариант, который читается дальше всего.
fn parse_track(data: &[u8]) -> (RawTrack, Option<String>) {
    let mut short = BTreeSet::new();
    let mut best = parse_from(data, Cursor::default(), Vec::new(), Vec::new(), &short, usize::MAX);
    let mut fixes = 0usize;
    while best.error.is_some() && fixes < 64 {
        let fail_pos = best.cur.pos;
        let first = best.checkpoints.len().saturating_sub(128);
        let mut winner: Option<(usize, usize)> = None;
        for c in first..best.checkpoints.len() {
            let cp = best.checkpoints[c];
            if !cp.two_byte || short.contains(&c) {
                continue;
            }
            let mut trial_short = short.clone();
            trial_short.insert(c);
            let trial = parse_from(data, cp.at, Vec::new(), Vec::new(), &trial_short, fail_pos + 4096);
            let reached = if trial.finished { usize::MAX } else { trial.cur.pos };
            if reached > fail_pos && winner.is_none_or(|(_, r)| reached > r) {
                winner = Some((c, reached));
            }
        }
        let Some((c, _)) = winner else { break };
        short.insert(c);
        let cp = best.checkpoints[c];
        let mut events = best.events;
        events.truncate(cp.at.emitted);
        let mut checkpoints = best.checkpoints;
        checkpoints.truncate(c);
        best = parse_from(data, cp.at, events, checkpoints, &short, usize::MAX);
        fixes += 1;
    }
    let mut notes = Vec::new();
    if let Some(e) = best.error {
        notes.push(format!("обрезана на событии {}: {e}", best.cur.index));
    }
    if fixes > 0 {
        notes.push(format!("исправлено сообщений с пропущенным байтом: {fixes} (ошибка программы, создавшей файл)"));
    }
    if best.cur.system_skipped > 0 {
        notes.push(format!("пропущено системных сообщений, недопустимых в файле: {}", best.cur.system_skipped));
    }
    let problem = (!notes.is_empty()).then(|| notes.join("; "));
    (RawTrack { events: best.events }, problem)
}

fn parse_from(
    data: &[u8],
    start: Cursor,
    mut events: Vec<RawEvent>,
    mut checkpoints: Vec<Checkpoint>,
    short: &BTreeSet<usize>,
    stop_at: usize,
) -> Pass {
    let mut cur = start;
    macro_rules! fail {
        ($at:expr, $what:expr) => {
            return Pass { events, checkpoints, error: Some($what.to_string()), cur: $at, finished: false }
        };
    }
    while cur.pos < data.len() {
        if cur.pos >= stop_at {
            return Pass { events, checkpoints, error: None, cur, finished: false };
        }
        let at = cur;
        checkpoints.push(Checkpoint { at, two_byte: false });
        let mut pos = cur.pos;
        let Some(delta) = read_vlq(data, &mut pos) else { fail!(at, "битое время события") };
        let tick = cur.tick + delta as u64;
        let Some(&first) = data.get(pos) else { fail!(at, "файл закончился посреди события") };
        let status = if first & 0x80 != 0 {
            pos += 1;
            first
        } else if let Some(rs) = cur.running {
            rs
        } else {
            fail!(at, "байт данных без статуса")
        };
        let running: Option<u8>;
        let mut event = None;
        let mut eot = false;
        match status {
            0x80..=0xEF => {
                running = Some(status);
                let ch = status & 0x0F;
                let mut need = if matches!(status & 0xF0, 0xC0 | 0xD0) { 1 } else { 2 };
                checkpoints.last_mut().unwrap().two_byte = need == 2;
                let truncated = need == 2 && short.contains(&cur.index);
                if truncated {
                    need = 1;
                }
                let Some(d) = data.get(pos..pos + need) else { fail!(at, "файл закончился посреди сообщения") };
                if d.iter().any(|&b| b & 0x80 != 0) {
                    fail!(at, "оборванное сообщение");
                }
                pos += need;
                // Укороченное сообщение отбрасываем: его значение неизвестно.
                if !truncated {
                    event = Some(match status & 0xF0 {
                        0x80 => RawKind::NoteOff { ch, key: d[0] },
                        0x90 if d[1] == 0 => RawKind::NoteOff { ch, key: d[0] },
                        0x90 => RawKind::NoteOn { ch, key: d[0], vel: d[1] },
                        0xA0 => RawKind::Control { ch, kind: ControlKind::PolyPressure(d[0]), value: d[1] as u16 },
                        0xB0 => RawKind::Control { ch, kind: ControlKind::Cc(d[0]), value: d[1] as u16 },
                        0xC0 => RawKind::Control { ch, kind: ControlKind::Program, value: d[0] as u16 },
                        0xD0 => RawKind::Control { ch, kind: ControlKind::ChannelPressure, value: d[0] as u16 },
                        _ => RawKind::Control {
                            ch,
                            kind: ControlKind::PitchBend,
                            value: d[0] as u16 | ((d[1] as u16) << 7),
                        },
                    });
                }
            }
            0xF0 | 0xF7 => {
                running = None;
                let Some(len) = read_vlq(data, &mut pos) else { fail!(at, "битая длина SysEx") };
                let Some(payload) = data.get(pos..pos + len as usize) else { fail!(at, "SysEx длиннее дорожки") };
                pos += len as usize;
                if status == 0xF0 {
                    event = Some(RawKind::SysEx(payload.to_vec()));
                }
            }
            0xFF => {
                running = None;
                let Some(&mtype) = data.get(pos) else { fail!(at, "оборванное мета-событие") };
                pos += 1;
                let Some(len) = read_vlq(data, &mut pos) else { fail!(at, "битая длина мета-события") };
                let Some(p) = data.get(pos..pos + len as usize) else { fail!(at, "мета-событие длиннее дорожки") };
                pos += len as usize;
                let kind = meta_kind(mtype, p);
                eot = kind == RawKind::EndOfTrack;
                event = Some(kind);
            }
            // Системные сообщения в SMF недопустимы, но встречаются — пропускаем их данные.
            0xF1 | 0xF3 => {
                pos += 1;
                running = None;
                cur.system_skipped += 1;
            }
            0xF2 => {
                pos += 2;
                running = None;
                cur.system_skipped += 1;
            }
            _ => fail!(at, format!("недопустимый статус 0x{status:02X}")),
        }
        if let Some(kind) = event {
            events.push(RawEvent { tick, kind });
        }
        cur = Cursor { pos, tick, running, index: cur.index + 1, emitted: events.len(), system_skipped: cur.system_skipped };
        if eot {
            break;
        }
    }
    Pass { events, checkpoints, error: None, cur, finished: true }
}

fn meta_kind(mtype: u8, p: &[u8]) -> RawKind {
    match (mtype, p.len()) {
        (0x51, 3) => RawKind::Meta(MetaKind::Tempo(((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32)),
        (0x58, 4) => RawKind::Meta(MetaKind::TimeSignature([p[0], p[1], p[2], p[3]])),
        (0x59, 2) => RawKind::Meta(MetaKind::KeySignature(p[0] as i8, p[1] != 0)),
        (0x03, _) => RawKind::TrackName(p.to_vec()),
        (0x2F, _) => RawKind::EndOfTrack,
        _ => RawKind::OtherMeta,
    }
}

fn is_known_meta(mtype: u8) -> bool {
    matches!(mtype, 0x00..=0x0F | 0x20 | 0x21 | 0x2F | 0x51 | 0x54 | 0x58 | 0x59 | 0x7F)
}

fn has_audible_notes(track: &RawTrack) -> bool {
    track.events.iter().any(|e| matches!(e.kind, RawKind::NoteOn { .. }))
}

/// Некоторые редакторы с несколькими MIDI-портами пишут номер канала k > 15 прямо в байт статуса,
/// и он переполняется в соседние типы сообщений: программа `0xC0 + k % 64`, note_on `0x90 + k`,
/// note_off `0x80 + k` (по модулю 128). Стандартный разбор такой дорожки даёт мусор: ложные
/// контроллеры и pitch bend на настоящих каналах и время событий далеко за концом песни.
/// Дорожка распознаётся, только если вся читается по этой схеме, начинается со смены программы
/// и содержит ноты.
/// Ноты попадают на канал `k % 16`, смена программы отбрасывается.
fn parse_overflowed(data: &[u8]) -> Option<RawTrack> {
    let mut pos = 0;
    let first = loop {
        read_vlq(data, &mut pos)?;
        let status = *data.get(pos)?;
        if status != 0xFF || !is_known_meta(*data.get(pos + 1)?) {
            break status;
        }
        pos += 2;
        let len = read_vlq(data, &mut pos)? as usize;
        pos += len;
    };
    if first < 0xC0 {
        return None;
    }
    let low = first - 0xC0;
    [low, low + 64].into_iter().filter(|k| (16..128).contains(k)).find_map(|k| parse_overflowed_as(data, k))
}

fn parse_overflowed_as(data: &[u8], k: u8) -> Option<RawTrack> {
    let program = 0xC0 | (k & 0x3F);
    let on = 0x80 | (k.wrapping_add(0x10) & 0x7F);
    let off = 0x80 | (k & 0x7F);
    let ch = k % 16;
    let mut events = Vec::new();
    let (mut pos, mut tick, mut started) = (0usize, 0u64, false);
    while pos < data.len() {
        tick += read_vlq(data, &mut pos)? as u64;
        let status = *data.get(pos)?;
        if status & 0x80 == 0 {
            return None;
        }
        pos += 1;
        let is_note = started && (status == on || status == off);
        // 0xFF может оказаться программой или нотой; конец дорожки — только последние «ff 2f 00».
        let is_meta = status == 0xFF
            && if started {
                !is_note || data.get(pos..) == Some(&[0x2F, 0x00][..])
            } else {
                status != program || data.get(pos).is_some_and(|&t| is_known_meta(t))
            };
        if is_meta {
            let mtype = *data.get(pos)?;
            pos += 1;
            let len = read_vlq(data, &mut pos)? as usize;
            let kind = meta_kind(mtype, data.get(pos..pos + len)?);
            pos += len;
            let eot = kind == RawKind::EndOfTrack;
            events.push(RawEvent { tick, kind });
            if eot {
                break;
            }
        } else if !started {
            if status != program || data.get(pos).is_none_or(|b| b & 0x80 != 0) {
                return None;
            }
            pos += 1;
            started = true;
        } else if is_note {
            let d = data.get(pos..pos + 2)?;
            if d.iter().any(|b| b & 0x80 != 0) {
                return None;
            }
            pos += 2;
            let kind = if status == on && d[1] > 0 { RawKind::NoteOn { ch, key: d[0], vel: d[1] } } else { RawKind::NoteOff { ch, key: d[0] } };
            events.push(RawEvent { tick, kind });
        } else {
            return None;
        }
    }
    // Одной смены программы мало, чтобы признать запись нестандартной: нужны ноты.
    let has_notes = events.iter().any(|e| matches!(e.kind, RawKind::NoteOn { .. } | RawKind::NoteOff { .. }));
    (has_notes && pos == data.len()).then_some(RawTrack { events })
}

/// Дорожки, где есть ноты, но ни одна не звучит (velocity 0) — например, рисунок в пианоролле.
/// Их канальные события убираются: смены программы и контроллеры из них перебивали бы настоящие
/// партии на тех же каналах. Возвращает число таких дорожек.
fn silence_ghost_tracks(tracks: &mut [RawTrack]) -> usize {
    let mut count = 0;
    for t in tracks {
        let has_offs = t.events.iter().any(|e| matches!(e.kind, RawKind::NoteOff { .. }));
        if has_offs && !has_audible_notes(t) {
            t.events.retain(|e| !matches!(e.kind, RawKind::NoteOff { .. } | RawKind::Control { .. }));
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{SmfBuilder, TrackBuilder};

    fn simple() -> Vec<u8> {
        SmfBuilder::new(480)
            .track(TrackBuilder::new().name("Piano").tempo(0, 600_000).on(0, 0, 60, 100).off(480, 0, 60))
            .bytes()
    }

    #[test]
    fn reads_valid_file() {
        let raw = read_midi(&simple()).unwrap();
        assert_eq!(raw.format, 1);
        assert_eq!(raw.division, Division::Ticks(480));
        assert!(raw.problems.is_empty());
        let ev = &raw.tracks[0].events;
        assert!(ev.contains(&RawEvent { tick: 0, kind: RawKind::TrackName(b"Piano".to_vec()) }));
        assert!(ev.contains(&RawEvent { tick: 0, kind: RawKind::Meta(MetaKind::Tempo(600_000)) }));
        assert!(ev.contains(&RawEvent { tick: 0, kind: RawKind::NoteOn { ch: 0, key: 60, vel: 100 } }));
        assert!(ev.contains(&RawEvent { tick: 480, kind: RawKind::NoteOff { ch: 0, key: 60 } }));
    }

    #[test]
    fn note_on_zero_velocity_is_note_off() {
        let bytes = SmfBuilder::new(96).track(TrackBuilder::new().on(0, 2, 40, 90).on(10, 2, 40, 0)).bytes();
        let raw = read_midi(&bytes).unwrap();
        assert!(raw.tracks[0].events.contains(&RawEvent { tick: 10, kind: RawKind::NoteOff { ch: 2, key: 40 } }));
    }

    #[test]
    fn running_status() {
        // Дорожка вручную: note on, затем две ноты с running status.
        let track = [0x00, 0x90, 60, 100, 0x10, 62, 100, 0x10, 60, 0];
        let bytes = SmfBuilder::new(96).raw_track(&track).bytes();
        let raw = read_midi(&bytes).unwrap();
        let ev = &raw.tracks[0].events;
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[1], RawEvent { tick: 16, kind: RawKind::NoteOn { ch: 0, key: 62, vel: 100 } });
        assert_eq!(ev[2], RawEvent { tick: 32, kind: RawKind::NoteOff { ch: 0, key: 60 } });
    }

    #[test]
    fn not_midi() {
        assert_eq!(read_midi(b"hello world, definitely not midi"), Err(ReadError::NotMidi));
        assert_eq!(read_midi(b"MThd"), Err(ReadError::NotMidi));
    }

    #[test]
    fn truncated_file_keeps_events_and_reports() {
        let mut bytes = simple();
        bytes.truncate(bytes.len() - 6);
        let raw = read_midi(&bytes).unwrap();
        assert!(raw.recovered());
        assert!(raw.problems.iter().any(|p| p.contains("длина выходит за конец")), "{:?}", raw.problems);
        assert!(raw.tracks[0].events.iter().any(|e| matches!(e.kind, RawKind::NoteOn { .. })));
    }

    #[test]
    fn broken_format_field() {
        let mut bytes = simple();
        bytes[9] = 7;
        let raw = read_midi(&bytes).unwrap();
        assert_eq!(raw.format, 1);
        assert!(raw.problems[0].contains("формат 7"));
    }

    #[test]
    fn garbage_between_chunks() {
        let a = SmfBuilder::new(96).track(TrackBuilder::new().on(0, 0, 60, 100).off(10, 0, 60)).bytes();
        let b = SmfBuilder::new(96).track(TrackBuilder::new().on(0, 1, 50, 100).off(10, 1, 50)).bytes();
        let mut bytes = a.clone();
        bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x01]);
        bytes.extend_from_slice(&b[14..]);
        let raw = read_midi(&bytes).unwrap();
        assert_eq!(raw.tracks.len(), 2);
        assert!(raw.problems.iter().any(|p| p.contains("мусора")));
    }

    #[test]
    fn invalid_status_cuts_track() {
        let track = [0x00, 0x90, 60, 100, 0x00, 0xF4, 0x10, 0x80, 60, 0];
        let raw = read_midi(&SmfBuilder::new(96).raw_track(&track).bytes()).unwrap();
        assert_eq!(raw.tracks[0].events.len(), 1);
        assert!(raw.problems[0].contains("0xF4"));
    }

    #[test]
    fn one_byte_pitch_bend_is_repaired() {
        // Как в KICK BACK: «e0 68» без второго байта, дальше нормальные события.
        let mut track = vec![0x00, 0xE0, 0x68];
        for i in 0..20u8 {
            track.extend_from_slice(&[0x00, 0xB0, 0x50 + (i % 3), 0x00, 0x10, 0x90, 60 + i, 100, 0x10, 0x80, 60 + i, 0]);
        }
        let raw = read_midi(&SmfBuilder::new(96).raw_track(&track).bytes()).unwrap();
        let notes = raw.tracks[0].events.iter().filter(|e| matches!(e.kind, RawKind::NoteOn { .. })).count();
        assert_eq!(notes, 20);
        assert_eq!(raw.tracks[0].events.len(), 60, "{:?}", raw.problems);
        assert!(raw.problems[0].contains("пропущенным байтом: 1"), "{:?}", raw.problems);
        assert_eq!(raw.tracks[0].events[1], RawEvent { tick: 16, kind: RawKind::NoteOn { ch: 0, key: 60, vel: 100 } });
    }

    #[test]
    fn overflowed_channel_track_is_decoded() {
        // Канал 33 (k = 32): программа 0xE0, note_on 0xB0, note_off 0xA0 — как в «UNDERTALE CORE (400 SEQUENCE SPECIAL)».
        let track = [
            0x00, 0xFF, 0x03, 0x02, b'S', b'w', 0x00, 0xE0, 0x51, 0x8A, 0x40, 0xB0, 0x32, 0x64, 0x81, 0x40, 0xA0, 0x32, 0x00, 0x00, 0xFF, 0x2F, 0x00,
        ];
        let raw = read_midi(&SmfBuilder::new(96).raw_track(&track).bytes()).unwrap();
        let ev = &raw.tracks[0].events;
        assert!(ev.contains(&RawEvent { tick: 1344, kind: RawKind::NoteOn { ch: 0, key: 0x32, vel: 0x64 } }), "{ev:?}");
        assert!(ev.contains(&RawEvent { tick: 1536, kind: RawKind::NoteOff { ch: 0, key: 0x32 } }));
        assert!(!ev.iter().any(|e| matches!(e.kind, RawKind::Control { .. })));
        assert_eq!(raw.end_tick(), 1536);
        assert!(raw.problems.iter().any(|p| p.contains("больше 16: 1")), "{:?}", raw.problems);
    }

    #[test]
    fn overflowed_status_0xff_is_a_note_until_the_last_event() {
        // k = 127: программа 0xFF, note_off тоже 0xFF; последние «ff 2f 00» — конец дорожки.
        let track = [0x00, 0xFF, 0x03, 0x01, b'x', 0x00, 0xFF, 0x50, 0x10, 0xFF, 0x34, 0x00, 0x60, 0xFF, 0x34, 0x00, 0x00, 0xFF, 0x2F, 0x00];
        let raw = read_midi(&SmfBuilder::new(96).raw_track(&track).bytes()).unwrap();
        // Только беззвучные ноты — после чтения канальных событий не остаётся.
        assert!(raw.tracks[0].events.iter().all(|e| !matches!(e.kind, RawKind::NoteOn { .. } | RawKind::NoteOff { .. } | RawKind::Control { .. })));
        assert_eq!(raw.end_tick(), 112);
    }

    #[test]
    fn overflowed_track_readable_as_system_messages_is_still_decoded() {
        // k = 49: программа 0xF1 читается стандартно как MTC (1 байт), а ноты 0xB1 — как CC на канале 2.
        let track = [0x00, 0xF1, 0x50, 0x8A, 0x20, 0xB1, 0x4F, 0x00, 0x60, 0xB1, 0x4F, 0x00, 0x00, 0xFF, 0x2F, 0x00];
        let raw = read_midi(&SmfBuilder::new(96).raw_track(&track).bytes()).unwrap();
        assert!(!raw.tracks[0].events.iter().any(|e| matches!(e.kind, RawKind::Control { .. })), "{:?}", raw.tracks[0].events);
    }

    #[test]
    fn ghost_track_does_not_override_program() {
        let real = TrackBuilder::new().program(0, 13, 5).note(0, 96, 13, 60, 100);
        // Дорожка-рисунок: та же программа на канале 13 и ноты с velocity 0.
        let ghost = TrackBuilder::new().program(0, 13, 80).on(10, 13, 74, 0).off(20, 13, 74).cc(30, 13, 64, 127);
        let raw = read_midi(&SmfBuilder::new(96).track(real).track(ghost).bytes()).unwrap();
        assert!(!raw.tracks[1].events.iter().any(|e| matches!(e.kind, RawKind::Control { .. } | RawKind::NoteOff { .. })));
        assert!(raw.tracks[0].events.iter().any(|e| matches!(e.kind, RawKind::Control { ch: 13, value: 5, .. })));
        assert!(raw.problems.iter().any(|p| p.contains("беззвучными нотами: 1")), "{:?}", raw.problems);
    }

    #[test]
    fn normal_tracks_are_not_taken_for_overflowed() {
        // Обычная дорожка с канальным давлением в начале и нотами на канале 0 не трогается.
        let t = TrackBuilder::new().event(0, &[0xD0, 0x40]).note(0, 96, 0, 60, 100).note(96, 192, 0, 62, 100);
        let raw = read_midi(&SmfBuilder::new(96).track(t).bytes()).unwrap();
        assert!(raw.problems.is_empty(), "{:?}", raw.problems);
        assert_eq!(raw.tracks[0].events.iter().filter(|e| matches!(e.kind, RawKind::NoteOn { ch: 0, .. })).count(), 2);
    }

    #[test]
    fn smpte_division() {
        let mut bytes = simple();
        bytes[12] = (-25i8) as u8;
        bytes[13] = 40;
        let raw = read_midi(&bytes).unwrap();
        assert_eq!(raw.division, Division::Smpte { fps: 25, ticks_per_frame: 40 });
    }

    #[test]
    fn riff_wrapper() {
        let smf = simple();
        let mut riff = b"RIFF".to_vec();
        riff.extend_from_slice(&((smf.len() + 12) as u32).to_le_bytes());
        riff.extend_from_slice(b"RMIDdata");
        riff.extend_from_slice(&(smf.len() as u32).to_le_bytes());
        riff.extend_from_slice(&smf);
        assert!(read_midi(&riff).unwrap().problems.is_empty());
    }
}
