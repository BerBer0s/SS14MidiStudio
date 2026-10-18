//! Шаги обработки песни. Каждый шаг меняет `Song` на месте и возвращает статистику.

pub mod cleanup;
pub mod limiter;
pub mod trim;
pub mod velocity;
