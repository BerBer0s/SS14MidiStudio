//! Модель песни → SMF формата 1: дорожка темпа + по дорожке на каждый непустой канал.

use crate::build::{MAX_CHANNEL_NAME, truncate_chars};
use crate::model::{ControlKind, Division, MetaKind, Song, CHANNELS};

pub fn export_smf(song: &Song) -> Vec<u8> {
    let mut tracks: Vec<Vec<u8>> = Vec::new();
    tracks.push(tempo_track(song));
    for ch in 0..CHANNELS {
        if !song.channels[ch].is_empty() {
            tracks.push(channel_track(song, ch));
        }
    }
    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    let division = match song.division {
        Division::Ticks(t) => t,
        Division::Smpte { fps, ticks_per_frame } => (((-(fps as i8)) as u8 as u16) << 8) | ticks_per_frame as u16,
    };
    out.extend_from_slice(&division.to_be_bytes());
    for t in tracks {
        out.extend_from_slice(b"MTrk");
        out.extend_from_slice(&(t.len() as u32).to_be_bytes());
        out.extend_from_slice(&t);
    }
    out
}

struct Writer {
    out: Vec<u8>,
    last_tick: u64,
    running: Option<u8>,
}

impl Writer {
    fn new() -> Writer {
        Writer { out: Vec::new(), last_tick: 0, running: None }
    }

    fn delta(&mut self, tick: u64) {
        push_vlq(&mut self.out, (tick - self.last_tick).min(0x0FFF_FFFF) as u32);
        self.last_tick = tick;
    }

    fn channel(&mut self, tick: u64, status: u8, data: &[u8]) {
        self.delta(tick);
        if self.running != Some(status) {
            self.out.push(status);
            self.running = Some(status);
        }
        self.out.extend_from_slice(data);
    }

    fn meta(&mut self, tick: u64, kind: u8, data: &[u8]) {
        self.delta(tick);
        self.running = None;
        self.out.extend_from_slice(&[0xFF, kind]);
        push_vlq(&mut self.out, data.len() as u32);
        self.out.extend_from_slice(data);
    }

    fn sysex(&mut self, tick: u64, data: &[u8]) {
        self.delta(tick);
        self.running = None;
        self.out.push(0xF0);
        push_vlq(&mut self.out, data.len() as u32);
        self.out.extend_from_slice(data);
    }

    fn finish(mut self, end: u64) -> Vec<u8> {
        let end = end.max(self.last_tick);
        self.meta(end, 0x2F, &[]);
        self.out
    }
}

fn tempo_track(song: &Song) -> Vec<u8> {
    let mut w = Writer::new();
    w.meta(0, 0x03, b"Tempo");
    // Мета и SysEx вместе, по тику; мета раньше SysEx на одном тике.
    let mut items: Vec<(u64, u8, usize)> = song.meta.iter().enumerate().map(|(i, m)| (m.tick, 0, i)).collect();
    items.extend(song.sysex.iter().enumerate().map(|(i, s)| (s.tick, 1, i)));
    items.sort_unstable();
    for (tick, kind, i) in items {
        if kind == 1 {
            w.sysex(tick, &song.sysex[i].data);
            continue;
        }
        match song.meta[i].kind {
            MetaKind::Tempo(t) => {
                let b = t.min(0xFF_FFFF).to_be_bytes();
                w.meta(tick, 0x51, &b[1..]);
            }
            MetaKind::TimeSignature(ts) => w.meta(tick, 0x58, &ts),
            MetaKind::KeySignature(sf, minor) => w.meta(tick, 0x59, &[sf as u8, minor as u8]),
        }
    }
    w.finish(0)
}

fn channel_track(song: &Song, ch: usize) -> Vec<u8> {
    let channel = &song.channels[ch];
    let c = ch as u8;
    // (тик, класс: 0 — note off, 1 — контроллер, 2 — note on, порядок, статус, данные)
    let mut events: Vec<(u64, u8, usize, u8, [u8; 2], u8)> = Vec::with_capacity(channel.notes.len() * 2 + channel.controls.len());
    for (i, n) in channel.notes.iter().enumerate() {
        events.push((n.end, 0, i, 0x80 | c, [n.key, 0], 2));
        events.push((n.start, 2, i, 0x90 | c, [n.key, n.velocity.clamp(1, 127)], 2));
    }
    for (i, ctl) in channel.controls.iter().enumerate() {
        let v = ctl.value;
        let (status, data, len) = match ctl.kind {
            ControlKind::Program => (0xC0, [v.min(127) as u8, 0], 1),
            ControlKind::Cc(n) => (0xB0, [n, v.min(127) as u8], 2),
            ControlKind::PitchBend => (0xE0, [(v & 0x7F) as u8, ((v >> 7) & 0x7F) as u8], 2),
            ControlKind::ChannelPressure => (0xD0, [v.min(127) as u8, 0], 1),
            ControlKind::PolyPressure(k) => (0xA0, [k, v.min(127) as u8], 2),
        };
        events.push((ctl.tick, 1, i, status | c, data, len));
    }
    events.sort_unstable_by_key(|e| (e.0, e.1, e.2));

    let mut w = Writer::new();
    let name = truncate_chars(&song.channel_label(ch), MAX_CHANNEL_NAME);
    w.meta(0, 0x03, name.as_bytes());
    let mut end = 0;
    for (tick, _, _, status, data, len) in events {
        w.channel(tick, status, &data[..len as usize]);
        end = tick;
    }
    w.finish(end)
}

pub fn push_vlq(out: &mut Vec<u8>, mut value: u32) {
    let mut buf = [0u8; 5];
    let mut i = 4;
    buf[i] = (value & 0x7F) as u8;
    value >>= 7;
    while value > 0 {
        i -= 1;
        buf[i] = 0x80 | (value & 0x7F) as u8;
        value >>= 7;
    }
    out.extend_from_slice(&buf[i..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::build_song;
    use crate::model::{Control, MetaEvent, Note, SysEx};
    use crate::read::{read_midi, RawKind};

    fn sample() -> Song {
        let mut s = Song::empty(Division::Ticks(96));
        s.meta = vec![MetaEvent { tick: 0, kind: MetaKind::Tempo(400_000) }, MetaEvent { tick: 96, kind: MetaKind::Tempo(300_000) }];
        s.channels[0].name = Some("Мелодия".into());
        s.channels[0].notes = vec![
            Note { start: 0, end: 10, key: 60, velocity: 100 },
            Note { start: 10, end: 20, key: 60, velocity: 90 },
        ];
        s.channels[0].controls = vec![Control { tick: 10, kind: ControlKind::PitchBend, value: 9000 }];
        s.channels[9].notes = vec![Note { start: 5, end: 6, key: 36, velocity: 85 }];
        s.sysex = vec![SysEx { tick: 0, data: vec![0x7E, 0x7F, 0x09, 0x01, 0xF7] }];
        s
    }

    #[test]
    fn roundtrip_preserves_song() {
        let song = sample();
        let raw = read_midi(&export_smf(&song)).unwrap();
        assert!(raw.problems.is_empty());
        let (back, stats) = build_song(&raw);
        assert_eq!(back.channels[0].notes, song.channels[0].notes);
        assert_eq!(back.channels[0].controls, song.channels[0].controls);
        assert_eq!(back.channels[9].notes, song.channels[9].notes);
        assert_eq!(back.meta, song.meta);
        assert_eq!(back.sysex, song.sysex);
        assert_eq!(back.channels[0].name.as_deref(), Some("Мелодия"));
        assert_eq!(back.channels[9].name.as_deref(), Some("Drums"));
        assert_eq!(stats.orphan_offs + stats.overlaps_resolved + stats.hanging_closed, 0);
    }

    #[test]
    fn same_tick_order_off_controls_on() {
        let raw = read_midi(&export_smf(&sample())).unwrap();
        let ev: Vec<&RawKind> = raw.tracks[1].events.iter().filter(|e| e.tick == 10).map(|e| &e.kind).collect();
        assert!(matches!(ev[0], RawKind::NoteOff { .. }));
        assert!(matches!(ev[1], RawKind::Control { kind: ControlKind::PitchBend, .. }));
        assert!(matches!(ev[2], RawKind::NoteOn { .. }));
    }

    #[test]
    fn layout_is_tempo_track_plus_channels() {
        let raw = read_midi(&export_smf(&sample())).unwrap();
        assert_eq!(raw.format, 1);
        assert_eq!(raw.tracks.len(), 3);
        assert_eq!(raw.tracks[0].events[0].kind, RawKind::TrackName(b"Tempo".to_vec()));
    }

    #[test]
    fn smpte_division_roundtrip() {
        let mut s = sample();
        s.division = Division::Smpte { fps: 25, ticks_per_frame: 40 };
        assert_eq!(read_midi(&export_smf(&s)).unwrap().division, s.division);
    }
}
