//! Свойства на случайных «грязных» MIDI: пустые note_off, наложения, нулевые ноты, мусорные CC.

use proptest::prelude::*;
use ss14midi_core::build::build_song;
use ss14midi_core::pipeline::{load, process_bytes, Preset, Settings};
use ss14midi_core::read::read_midi;
use ss14midi_core::testkit::{SmfBuilder, TrackBuilder};
use ss14midi_core::Status;

#[derive(Clone, Debug)]
enum Ev {
    On(u64, u8, u8, u8),
    Off(u64, u8, u8),
    Cc(u64, u8, u8, u8),
    Bend(u64, u8, u16),
    Program(u64, u8, u8),
    Tempo(u64, u32),
}

fn ev() -> impl Strategy<Value = Ev> {
    let tick = 0u64..4000;
    let ch = prop_oneof![Just(0u8), Just(1), Just(9), 0u8..16];
    let key = 30u8..90;
    prop_oneof![
        6 => (tick.clone(), ch.clone(), key.clone(), 1u8..128).prop_map(|(t, c, k, v)| Ev::On(t, c, k, v)),
        6 => (tick.clone(), ch.clone(), key).prop_map(|(t, c, k)| Ev::Off(t, c, k)),
        2 => (tick.clone(), ch.clone(), 0u8..128, 0u8..128).prop_map(|(t, c, n, v)| Ev::Cc(t, c, n, v)),
        2 => (tick.clone(), ch.clone(), 0u16..16384).prop_map(|(t, c, v)| Ev::Bend(t, c, v)),
        1 => (tick.clone(), ch, 0u8..128).prop_map(|(t, c, p)| Ev::Program(t, c, p)),
        1 => (tick, 200_000u32..1_500_000).prop_map(|(t, us)| Ev::Tempo(t, us)),
    ]
}

fn smf() -> impl Strategy<Value = Vec<u8>> {
    (prop::collection::vec(prop::collection::vec(ev(), 0..400), 1..4), prop_oneof![Just(96u16), Just(480), Just(24)]).prop_map(
        |(tracks, tpq)| {
            let mut b = SmfBuilder::new(tpq);
            for evs in tracks {
                let mut t = TrackBuilder::new();
                for e in evs {
                    t = match e {
                        Ev::On(tick, c, k, v) => t.on(tick, c, k, v),
                        Ev::Off(tick, c, k) => t.off(tick, c, k),
                        Ev::Cc(tick, c, n, v) => t.cc(tick, c, n, v),
                        Ev::Bend(tick, c, v) => t.bend(tick, c, v),
                        Ev::Program(tick, c, p) => t.program(tick, c, p),
                        Ev::Tempo(tick, us) => t.tempo(tick, us),
                    };
                }
                b = b.track(t);
            }
            b.bytes()
        },
    )
}

fn check_clean(bytes: &[u8]) -> Result<(), TestCaseError> {
    let raw = read_midi(bytes).unwrap();
    prop_assert!(raw.problems.is_empty(), "{:?}", raw.problems);
    let (_, st) = build_song(&raw);
    prop_assert_eq!(st.orphan_offs, 0);
    prop_assert_eq!(st.hanging_closed, 0);
    prop_assert_eq!(st.overlaps_resolved + st.same_start_merged, 0);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn every_preset_produces_clean_file(bytes in smf()) {
        for preset in Preset::ALL {
            let p = process_bytes(&bytes, &Settings::new(preset)).unwrap();
            check_clean(&p.bytes)?;
        }
    }

    #[test]
    fn repair_keeps_notes_and_relative_timing(bytes in smf()) {
        let loaded = load(&bytes, &Settings::default().limits).unwrap();
        let p = process_bytes(&bytes, &Settings::new(Preset::Repair)).unwrap();
        prop_assert_eq!(p.song.note_count(), loaded.song.note_count());
        let starts = |s: &ss14midi_core::Song| {
            let mut v: Vec<(u64, usize, u8)> = s.channels.iter().enumerate()
                .flat_map(|(ch, c)| c.notes.iter().map(move |n| (n.start, ch, n.key))).collect();
            v.sort();
            v
        };
        let (a, b) = (starts(&loaded.song), starts(&p.song));
        if let (Some(fa), Some(fb)) = (a.first(), b.first()) {
            let shift = fb.0 as i64 - fa.0 as i64;
            for (x, y) in a.iter().zip(&b) {
                prop_assert_eq!(y.0 as i64 - x.0 as i64, shift);
                prop_assert_eq!((x.1, x.2), (y.1, y.2));
            }
        }
    }

    #[test]
    fn safe_is_ok_unless_unresolved(bytes in smf()) {
        let p = process_bytes(&bytes, &Settings::new(Preset::Safe)).unwrap();
        // Если не ОК — отчёт обязан показать, где перегрузка.
        prop_assert!(p.report.status == Status::Ok || !p.report.overloaded_ranges.is_empty(), "{:?}", p.report.after);
    }

    #[test]
    fn reader_never_panics(mut bytes in smf(), cut in 0usize..2000, flips in prop::collection::vec((0usize..4000, any::<u8>()), 0..20)) {
        for (pos, b) in flips {
            if pos + 14 < bytes.len() {
                bytes[pos + 14] = b;
            }
        }
        bytes.truncate(bytes.len().saturating_sub(cut).max(14));
        if let Ok(p) = process_bytes(&bytes, &Settings::default()) {
            check_clean(&p.bytes)?;
        }
    }
}
