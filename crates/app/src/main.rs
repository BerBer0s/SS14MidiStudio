#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! SS14 MIDI Studio — оптимизатор и редактор MIDI для инструментов Space Station 14.

mod app;
mod editor;
mod i18n;
mod icon_px;
mod library;
mod player;
mod theme;
mod views;
mod widgets;

fn main() -> eframe::Result {
    let icon = egui::IconData { rgba: icon_px::icon_rgba(64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SS14 MIDI Studio")
            .with_inner_size([1360.0, 840.0])
            .with_min_inner_size([980.0, 620.0])
            .with_icon(std::sync::Arc::new(icon))
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("SS14 MIDI Studio", options, Box::new(|cc| Ok(Box::new(app::StudioApp::new(cc)))))
}
