//! Проигрыватель: отдельный поток, вывод в системный MIDI-синтезатор (на Windows —
//! Microsoft GS Wavetable Synth, тот же набор gm.dls, что использует SS14).
//! Режим «как слышат другие» повторяет пакетную отправку клиента SS14: не больше 60 событий
//! за кадр и 1000 в секунду; всё сверх — откладывается.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use midir::{MidiOutput, MidiOutputConnection};
use ss14midi_core::model::ControlKind;
use ss14midi_core::{Limits, Song};

#[derive(Clone, Copy, Debug)]
pub struct PlayEvent {
    pub us: u64,
    pub data: [u8; 3],
    pub len: u8,
}

impl PlayEvent {
    fn channel(&self) -> usize {
        (self.data[0] & 0x0F) as usize
    }
    fn is_note_on(&self) -> bool {
        self.data[0] & 0xF0 == 0x90 && self.data[2] > 0
    }
    fn is_control(&self) -> bool {
        matches!(self.data[0] & 0xF0, 0xB0 | 0xC0 | 0xE0 | 0xD0)
    }
}

/// События песни во времени, в порядке как при экспорте.
pub fn build_events(song: &Song) -> Vec<PlayEvent> {
    let map = song.tempo_map();
    let mut ev: Vec<(u64, u8, PlayEvent)> = Vec::new();
    for (ch, c) in song.channels.iter().enumerate() {
        let chb = ch as u8;
        for n in &c.notes {
            ev.push((n.end, 0, PlayEvent { us: map.tick_to_us(n.end), data: [0x80 | chb, n.key, 0], len: 3 }));
            ev.push((n.start, 2, PlayEvent { us: map.tick_to_us(n.start), data: [0x90 | chb, n.key, n.velocity.max(1)], len: 3 }));
        }
        for x in &c.controls {
            let v = x.value;
            let (data, len) = match x.kind {
                ControlKind::Program => ([0xC0 | chb, v.min(127) as u8, 0], 2),
                ControlKind::Cc(n) => ([0xB0 | chb, n, v.min(127) as u8], 3),
                ControlKind::PitchBend => ([0xE0 | chb, (v & 0x7F) as u8, ((v >> 7) & 0x7F) as u8], 3),
                ControlKind::ChannelPressure => ([0xD0 | chb, v.min(127) as u8, 0], 2),
                ControlKind::PolyPressure(k) => ([0xA0 | chb, k, v.min(127) as u8], 3),
            };
            ev.push((x.tick, 1, PlayEvent { us: map.tick_to_us(x.tick), data, len }));
        }
    }
    ev.sort_by_key(|e| (e.0, e.1));
    ev.into_iter().map(|e| e.2).collect()
}

enum Cmd {
    Play { events: Arc<Vec<PlayEvent>>, from_us: u64 },
    Stop,
    Mute(u16),
    Simulate(bool),
    Port(String),
    RefreshPorts,
}

#[derive(Default)]
pub struct Shared {
    pub playing: AtomicBool,
    pub position_us: AtomicU64,
    /// Отставание очереди в режиме «как слышат другие».
    pub lag_us: AtomicU64,
    pub max_lag_us: AtomicU64,
    pub port: Mutex<String>,
    pub ports: Mutex<Vec<String>>,
    pub error: Mutex<Option<String>>,
}

pub struct Player {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
}

impl Player {
    pub fn new(preferred_port: Option<String>, repaint: impl Fn() + Send + 'static) -> Player {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        std::thread::Builder::new()
            .name("midi-player".into())
            .spawn(move || run(rx, s, preferred_port, repaint))
            .expect("поток проигрывателя");
        Player { tx, shared }
    }

    pub fn play(&self, events: Arc<Vec<PlayEvent>>, from_us: u64) {
        let _ = self.tx.send(Cmd::Play { events, from_us });
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    pub fn set_mute(&self, mask: u16) {
        let _ = self.tx.send(Cmd::Mute(mask));
    }
    pub fn set_simulate(&self, on: bool) {
        let _ = self.tx.send(Cmd::Simulate(on));
    }
    pub fn set_port(&self, name: String) {
        let _ = self.tx.send(Cmd::Port(name));
    }
    pub fn refresh_ports(&self) {
        let _ = self.tx.send(Cmd::RefreshPorts);
    }
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }
    pub fn position_us(&self) -> u64 {
        self.shared.position_us.load(Ordering::Relaxed)
    }
}

struct Output {
    conn: Option<MidiOutputConnection>,
}

impl Output {
    fn send(&mut self, bytes: &[u8]) {
        if let Some(c) = &mut self.conn {
            let _ = c.send(bytes);
        }
    }

    fn silence(&mut self) {
        for ch in 0..16u8 {
            self.send(&[0xB0 | ch, 64, 0]);
            self.send(&[0xB0 | ch, 123, 0]);
            self.send(&[0xB0 | ch, 120, 0]);
            self.send(&[0xE0 | ch, 0, 64]);
        }
    }
}

fn list_ports() -> Vec<String> {
    let Ok(out) = MidiOutput::new("SS14 MIDI Studio") else { return Vec::new() };
    out.ports().iter().filter_map(|p| out.port_name(p).ok()).collect()
}

fn connect(name: Option<&str>, shared: &Shared) -> Option<MidiOutputConnection> {
    let out = match MidiOutput::new("SS14 MIDI Studio") {
        Ok(o) => o,
        Err(e) => {
            *shared.error.lock().unwrap() = Some(e.to_string());
            return None;
        }
    };
    let ports = out.ports();
    let names: Vec<String> = ports.iter().map(|p| out.port_name(p).unwrap_or_default()).collect();
    *shared.ports.lock().unwrap() = names.clone();
    let pick = name
        .and_then(|n| names.iter().position(|x| x == n))
        .or_else(|| names.iter().position(|x| x.contains("GS Wavetable")))
        .or(if names.is_empty() { None } else { Some(0) })?;
    match out.connect(&ports[pick], "ss14-midi-studio") {
        Ok(c) => {
            *shared.port.lock().unwrap() = names[pick].clone();
            *shared.error.lock().unwrap() = None;
            Some(c)
        }
        Err(e) => {
            *shared.error.lock().unwrap() = Some(e.to_string());
            None
        }
    }
}

fn run(rx: Receiver<Cmd>, shared: Arc<Shared>, preferred: Option<String>, repaint: impl Fn()) {
    let mut out = Output { conn: connect(preferred.as_deref(), &shared) };
    let limits = Limits::default();
    let frame = Duration::from_micros(limits.frame_us());
    let mut events: Arc<Vec<PlayEvent>> = Arc::new(Vec::new());
    let mut idx = 0usize;
    let mut start = Instant::now();
    let mut start_us = 0u64;
    let mut playing = false;
    let mut mute: u16 = 0;
    let mut simulate = false;
    // Состояние эмуляции клиента SS14.
    let mut backlog: VecDeque<PlayEvent> = VecDeque::new();
    let mut next_frame = Instant::now();
    let mut second_start = Instant::now();
    let mut sent_in_second = 0u32;
    let mut last_repaint = Instant::now();

    loop {
        let now_us = |start: Instant, start_us: u64| start_us + start.elapsed().as_micros() as u64;
        let timeout = if playing {
            let next = events.get(idx).map(|e| e.us.saturating_sub(now_us(start, start_us))).unwrap_or(0);
            let mut t = Duration::from_micros(next.min(4000));
            if simulate {
                t = t.min(next_frame.saturating_duration_since(Instant::now()));
            }
            Some(t)
        } else {
            None
        };
        let cmd = match timeout {
            Some(t) => match rx.recv_timeout(t) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
        };
        match cmd {
            Some(Cmd::Play { events: ev, from_us }) => {
                out.silence();
                events = ev;
                idx = events.partition_point(|e| e.us < from_us);
                // Восстановить инструменты и контроллеры на момент старта.
                let mut state: Vec<[u8; 3]> = Vec::new();
                for e in &events[..idx] {
                    if e.is_control() {
                        state.retain(|s| !(s[0] == e.data[0] && (e.data[0] & 0xF0 != 0xB0 || s[1] == e.data[1])));
                        state.push(e.data);
                    }
                }
                for s in &state {
                    let len = if matches!(s[0] & 0xF0, 0xC0 | 0xD0) { 2 } else { 3 };
                    out.send(&s[..len]);
                }
                start = Instant::now();
                start_us = from_us;
                playing = true;
                backlog.clear();
                next_frame = Instant::now();
                second_start = Instant::now();
                sent_in_second = 0;
                shared.max_lag_us.store(0, Ordering::Relaxed);
                shared.lag_us.store(0, Ordering::Relaxed);
                shared.playing.store(true, Ordering::Relaxed);
            }
            Some(Cmd::Stop) => {
                playing = false;
                backlog.clear();
                out.silence();
                shared.playing.store(false, Ordering::Relaxed);
                repaint();
            }
            Some(Cmd::Mute(m)) => {
                let newly = m & !mute;
                mute = m;
                for ch in 0..16u8 {
                    if newly & (1 << ch) != 0 {
                        out.send(&[0xB0 | ch, 123, 0]);
                    }
                }
            }
            Some(Cmd::Simulate(on)) => {
                simulate = on;
                backlog.clear();
                shared.lag_us.store(0, Ordering::Relaxed);
            }
            Some(Cmd::Port(name)) => {
                out.silence();
                out.conn = None;
                out.conn = connect(Some(&name), &shared);
            }
            Some(Cmd::RefreshPorts) => {
                *shared.ports.lock().unwrap() = list_ports();
            }
            None => {}
        }
        if !playing {
            continue;
        }
        let now = now_us(start, start_us);
        while idx < events.len() && events[idx].us <= now {
            let e = events[idx];
            idx += 1;
            if e.is_note_on() && mute & (1 << e.channel()) != 0 {
                continue;
            }
            if simulate {
                backlog.push_back(e);
            } else {
                out.send(&e.data[..e.len as usize]);
            }
        }
        if simulate {
            let t = Instant::now();
            if t.duration_since(second_start) >= Duration::from_secs(1) {
                second_start = t;
                sent_in_second = 0;
            }
            if t >= next_frame {
                next_frame = t + frame;
                let budget = limits.frame_limit.min(limits.max_per_second.saturating_sub(sent_in_second));
                for _ in 0..budget {
                    let Some(e) = backlog.pop_front() else { break };
                    out.send(&e.data[..e.len as usize]);
                    sent_in_second += 1;
                }
            }
            let lag = backlog.front().map(|e| now.saturating_sub(e.us)).unwrap_or(0);
            shared.lag_us.store(lag, Ordering::Relaxed);
            shared.max_lag_us.fetch_max(lag, Ordering::Relaxed);
        }
        shared.position_us.store(now, Ordering::Relaxed);
        if idx >= events.len() && backlog.is_empty() {
            playing = false;
            out.silence();
            shared.playing.store(false, Ordering::Relaxed);
            repaint();
        }
        if last_repaint.elapsed() >= Duration::from_millis(30) {
            last_repaint = Instant::now();
            repaint();
        }
    }
}
