//! Цвета, шрифты и стиль egui.

use std::sync::Arc;

use egui::epaint::{CornerRadius, Margin, Shadow, Stroke};
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle, Vec2};
use ss14midi_core::Status;

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const BG: Color32 = hex(0x131518);
pub const PANEL: Color32 = hex(0x1A1D21);
pub const RAISED: Color32 = hex(0x23272C);
pub const HOVER: Color32 = hex(0x2C3137);
pub const LINE: Color32 = hex(0x2C3036);
pub const LINE_STRONG: Color32 = hex(0x3D434A);
pub const TEXT: Color32 = hex(0xE6E6E3);
pub const TEXT_DIM: Color32 = hex(0x9AA0A7);
pub const TEXT_FAINT: Color32 = hex(0x676D75);
pub const ACCENT: Color32 = hex(0xFFD21F);
pub const ACCENT_HOVER: Color32 = hex(0xFFE066);
/// Текст на жёлтом и на цветных плашках.
pub const INK: Color32 = hex(0x111111);
pub const OK: Color32 = hex(0x5DD47B);
pub const WARN: Color32 = hex(0xFFA23A);
pub const OVER: Color32 = hex(0xFF5A4E);

pub const RADIUS: u8 = 4;

/// Цвета каналов: различимы на тёмном фоне и не спорят с жёлтым и красным.
const CHANNEL_COLORS: [Color32; 16] = [
    hex(0x4FC3F7),
    hex(0xFF8A65),
    hex(0xAED581),
    hex(0xBA68C8),
    hex(0x4DB6AC),
    hex(0xF06292),
    hex(0x90A4AE),
    hex(0x7986CB),
    hex(0xA1887F),
    hex(0xE6E1D3),
    hex(0x81C784),
    hex(0x64B5F6),
    hex(0xFFB74D),
    hex(0x9575CD),
    hex(0x4DD0E1),
    hex(0xDCE775),
];

pub fn channel_color(ch: usize) -> Color32 {
    CHANNEL_COLORS[ch % 16]
}

pub fn status_color(s: Status) -> Color32 {
    match s {
        Status::Ok => OK,
        Status::Warn => WARN,
        Status::Over => OVER,
    }
}

const MONO: &str = "mono";
const BOLD: &str = "bold";

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO.into()))
}

pub fn ui_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

fn system_font(name: &str) -> Option<Vec<u8>> {
    let root = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
    std::fs::read(root.join("Fonts").join(name)).ok()
}

/// Семейство: сначала системный шрифт Windows, если он есть, затем шрифты egui.
fn family(fonts: &mut FontDefinitions, file: &str, fallback: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(bytes) = system_font(file) {
        fonts.font_data.insert(file.into(), Arc::new(FontData::from_owned(bytes)));
        out.push(file.to_string());
    }
    out.extend(fallback.iter().cloned());
    out
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mono_default = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();

    let ui = family(&mut fonts, "segoeui.ttf", &prop);
    let bold_family = family(&mut fonts, "seguisb.ttf", &ui);
    let mono_family = family(&mut fonts, "consola.ttf", &[mono_default, prop].concat());
    fonts.families.insert(FontFamily::Proportional, ui);
    fonts.families.insert(FontFamily::Name(BOLD.into()), bold_family);
    fonts.families.insert(FontFamily::Monospace, mono_family.clone());
    fonts.families.insert(FontFamily::Name(MONO.into()), mono_family);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, bold(19.0)),
            (TextStyle::Body, ui_font(14.0)),
            (TextStyle::Button, ui_font(14.0)),
            (TextStyle::Small, ui_font(12.0)),
            (TextStyle::Monospace, mono(13.0)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(8.0, 6.0);
        s.button_padding = Vec2::new(10.0, 4.0);
        s.interact_size = Vec2::new(28.0, 28.0);
        s.window_margin = Margin::same(14);
        s.menu_margin = Margin::same(6);
        s.combo_width = 160.0;

        let v = &mut style.visuals;
        let r = CornerRadius::same(RADIUS);
        v.dark_mode = true;
        v.override_text_color = None;
        v.weak_text_color = Some(TEXT_DIM);
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, LINE_STRONG);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = r;
        v.window_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) };
        v.popup_shadow = Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(90) };
        v.extreme_bg_color = hex(0x0F1113);
        v.faint_bg_color = RAISED;
        v.code_bg_color = RAISED;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = WARN;
        v.error_fg_color = OVER;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
        v.striped = false;

        let w = &mut v.widgets;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = r;
            wv.expansion = 0.0;
        }
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_fill = RAISED;
        w.inactive.weak_bg_fill = RAISED;
        w.inactive.bg_stroke = Stroke::new(1.0, LINE);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_fill = HOVER;
        w.hovered.weak_bg_fill = HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, LINE_STRONG);
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.bg_fill = HOVER;
        w.active.weak_bg_fill = HOVER;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.open.bg_fill = HOVER;
        w.open.weak_bg_fill = HOVER;
        w.open.bg_stroke = Stroke::new(1.0, LINE_STRONG);
        w.open.fg_stroke = Stroke::new(1.0, TEXT);
    });
}
