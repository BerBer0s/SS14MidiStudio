//! Конструктор SMF-файлов для тестов. Пишет байты напрямую, независимо от `export`,
//! чтобы тесты чтения не зависели от нашего же писателя.

use crate::export::push_vlq;

#[derive(Clone, Debug, Default)]
pub struct TrackBuilder {
    events: Vec<(u64, u64, Vec<u8>)>, // (тик, порядковый номер, байты события без delta)
}

impl TrackBuilder {
    pub fn new() -> TrackBuilder {
        TrackBuilder::default()
    }

    pub fn event(mut self, tick: u64, bytes: &[u8]) -> TrackBuilder {
        let seq = self.events.len() as u64;
        self.events.push((tick, seq, bytes.to_vec()));
        self
    }

    pub fn on(self, tick: u64, ch: u8, key: u8, vel: u8) -> TrackBuilder {
        self.event(tick, &[0x90 | ch, key, vel])
    }

    pub fn off(self, tick: u64, ch: u8, key: u8) -> TrackBuilder {
        self.event(tick, &[0x80 | ch, key, 0])
    }

    /// Нота целиком: note on + note off.
    pub fn note(self, start: u64, end: u64, ch: u8, key: u8, vel: u8) -> TrackBuilder {
        self.on(start, ch, key, vel).off(end, ch, key)
    }

    pub fn cc(self, tick: u64, ch: u8, cc: u8, value: u8) -> TrackBuilder {
        self.event(tick, &[0xB0 | ch, cc, value])
    }

    pub fn program(self, tick: u64, ch: u8, program: u8) -> TrackBuilder {
        self.event(tick, &[0xC0 | ch, program])
    }

    pub fn bend(self, tick: u64, ch: u8, value: u16) -> TrackBuilder {
        self.event(tick, &[0xE0 | ch, (value & 0x7F) as u8, (value >> 7) as u8])
    }

    pub fn tempo(self, tick: u64, us_per_quarter: u32) -> TrackBuilder {
        let b = us_per_quarter.to_be_bytes();
        self.event(tick, &[0xFF, 0x51, 0x03, b[1], b[2], b[3]])
    }

    pub fn name(self, name: &str) -> TrackBuilder {
        self.name_bytes(name.as_bytes())
    }

    pub fn name_bytes(self, name: &[u8]) -> TrackBuilder {
        let mut bytes = vec![0xFF, 0x03];
        push_vlq(&mut bytes, name.len() as u32);
        bytes.extend_from_slice(name);
        self.event(0, &bytes)
    }

    pub fn sysex(self, tick: u64, payload: &[u8]) -> TrackBuilder {
        let mut bytes = vec![0xF0];
        push_vlq(&mut bytes, payload.len() as u32);
        bytes.extend_from_slice(payload);
        self.event(tick, &bytes)
    }

    /// Байты дорожки (без заголовка чанка), события в порядке (тик, порядок добавления).
    pub fn encode(&self) -> Vec<u8> {
        let mut events = self.events.clone();
        events.sort_by_key(|e| (e.0, e.1));
        let mut out = Vec::new();
        let mut last = 0;
        for (tick, _, bytes) in &events {
            push_vlq(&mut out, (tick - last) as u32);
            out.extend_from_slice(bytes);
            last = *tick;
        }
        push_vlq(&mut out, 0);
        out.extend_from_slice(&[0xFF, 0x2F, 0x00]);
        out
    }
}

#[derive(Clone, Debug)]
pub struct SmfBuilder {
    format: u16,
    division: u16,
    tracks: Vec<Vec<u8>>,
}

impl SmfBuilder {
    pub fn new(ticks_per_quarter: u16) -> SmfBuilder {
        SmfBuilder { format: 1, division: ticks_per_quarter, tracks: Vec::new() }
    }

    pub fn format(mut self, format: u16) -> SmfBuilder {
        self.format = format;
        self
    }

    pub fn track(mut self, track: TrackBuilder) -> SmfBuilder {
        self.tracks.push(track.encode());
        self
    }

    /// Дорожка из готовых байт (без заголовка чанка и без End of Track).
    pub fn raw_track(mut self, bytes: &[u8]) -> SmfBuilder {
        self.tracks.push(bytes.to_vec());
        self
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = b"MThd".to_vec();
        out.extend_from_slice(&6u32.to_be_bytes());
        out.extend_from_slice(&self.format.to_be_bytes());
        out.extend_from_slice(&(self.tracks.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.division.to_be_bytes());
        for t in &self.tracks {
            out.extend_from_slice(b"MTrk");
            out.extend_from_slice(&(t.len() as u32).to_be_bytes());
            out.extend_from_slice(t);
        }
        out
    }
}
