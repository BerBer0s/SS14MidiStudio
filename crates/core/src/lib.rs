//! Ядро SS14 MIDI Studio: чтение, анализ и оптимизация MIDI под лимиты инструментов
//! Space Station 14.

pub mod analysis;
pub mod batch;
pub mod build;
pub mod export;
pub mod gm;
pub mod limits;
pub mod model;
pub mod passes;
pub mod pipeline;
pub mod read;
pub mod report;
#[doc(hidden)]
pub mod testkit;
pub mod timing;

pub use limits::{Limits, Status};
pub use model::{Channel, Control, ControlKind, Division, Note, Song};
pub use pipeline::{Preset, Settings};
