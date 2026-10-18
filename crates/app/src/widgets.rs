//! Кнопки, переключатели и векторные иконки. Иконки рисуются линиями, а не символами шрифта:
//! в системных шрифтах многих символов нет, и они превращаются в квадратики.

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use ss14midi_core::Status;

use crate::i18n;
use crate::theme::{self, bold, ui_font};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Undo,
    Redo,
    More,
    Fit,
    Plus,
    Minus,
    Eye,
    Trash,
}

pub fn paint_icon(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 2.0;
    let line = Stroke::new(1.6, color);
    let at = |x: f32, y: f32| c + Vec2::new(x * s, y * s);
    match icon {
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![at(-0.45, -0.6), at(0.65, 0.0), at(-0.45, 0.6)], color, Stroke::NONE));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(-0.5, -0.55), at(-0.12, 0.55)), 1.0, color);
            p.rect_filled(Rect::from_min_max(at(0.12, -0.55), at(0.5, 0.55)), 1.0, color);
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_min_max(at(-0.5, -0.5), at(0.5, 0.5)), 1.5, color);
        }
        Icon::Undo | Icon::Redo => {
            // Дуга со стрелкой вниз на конце: слева — отменить, справа — повторить.
            let m = if icon == Icon::Undo { 1.0 } else { -1.0 };
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    at(m * 0.5 * a.cos(), 0.25 - 0.55 * a.sin())
                })
                .collect();
            let tip = *pts.last().unwrap();
            p.add(Shape::line(pts, line));
            p.line_segment([tip, tip + Vec2::new(-0.25 * s, -0.25 * s)], line);
            p.line_segment([tip, tip + Vec2::new(0.25 * s, -0.25 * s)], line);
        }
        Icon::More => {
            for x in [-0.5, 0.0, 0.5] {
                p.circle_filled(at(x, 0.0), s * 0.13, color);
            }
        }
        Icon::Fit => {
            p.rect_stroke(Rect::from_min_max(at(-0.6, -0.45), at(0.6, 0.45)), 1.0, line, StrokeKind::Middle);
            p.line_segment([at(-0.3, 0.0), at(0.3, 0.0)], line);
            p.line_segment([at(-0.3, 0.0), at(-0.12, -0.16)], line);
            p.line_segment([at(-0.3, 0.0), at(-0.12, 0.16)], line);
            p.line_segment([at(0.3, 0.0), at(0.12, -0.16)], line);
            p.line_segment([at(0.3, 0.0), at(0.12, 0.16)], line);
        }
        Icon::Plus => {
            p.line_segment([at(-0.5, 0.0), at(0.5, 0.0)], line);
            p.line_segment([at(0.0, -0.5), at(0.0, 0.5)], line);
        }
        Icon::Minus => {
            p.line_segment([at(-0.5, 0.0), at(0.5, 0.0)], line);
        }
        Icon::Eye => {
            let mut pts = Vec::new();
            for i in 0..=16 {
                let x = -0.7 + 1.4 * i as f32 / 16.0;
                pts.push(at(x, -0.42 * (1.0 - (x / 0.7).powi(2)).sqrt()));
            }
            for i in (0..16).rev() {
                let x = -0.7 + 1.4 * i as f32 / 16.0;
                pts.push(at(x, 0.42 * (1.0 - (x / 0.7).powi(2)).sqrt()));
            }
            p.add(Shape::closed_line(pts, line));
            p.circle_filled(c, s * 0.2, color);
        }
        Icon::Trash => {
            p.line_segment([at(-0.55, -0.4), at(0.55, -0.4)], line);
            p.line_segment([at(-0.18, -0.4), at(-0.18, -0.6)], line);
            p.line_segment([at(-0.18, -0.6), at(0.18, -0.6)], line);
            p.line_segment([at(0.18, -0.6), at(0.18, -0.4)], line);
            p.add(Shape::closed_line(vec![at(-0.4, -0.4), at(-0.32, 0.6), at(0.32, 0.6), at(0.4, -0.4)], line));
        }
    }
}

fn frame_colors(resp: &Response, enabled: bool, selected: bool) -> (Color32, Color32, Color32) {
    if !enabled {
        (theme::PANEL, theme::LINE, theme::TEXT_FAINT)
    } else if selected {
        (theme::HOVER, theme::ACCENT, theme::TEXT)
    } else if resp.hovered() {
        (theme::HOVER, theme::LINE_STRONG, theme::TEXT)
    } else {
        (theme::RAISED, theme::LINE, theme::TEXT)
    }
}

/// Квадратная кнопка с иконкой.
pub fn icon_button(ui: &mut Ui, icon: Icon, hint: &str, enabled: bool) -> Response {
    icon_toggle(ui, icon, hint, enabled, false)
}

pub fn icon_toggle(ui: &mut Ui, icon: Icon, hint: &str, enabled: bool, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), if enabled { Sense::click() } else { Sense::hover() });
    let (fill, stroke, color) = frame_colors(&resp, enabled, selected);
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(theme::RADIUS), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    paint_icon(p, rect.shrink(7.0), icon, color);
    resp.on_hover_text(hint)
}

/// Обычная кнопка.
pub fn button(ui: &mut Ui, text: &str) -> Response {
    button_ex(ui, text, true, false)
}

pub fn button_ex(ui: &mut Ui, text: &str, enabled: bool, selected: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), ui_font(14.0), theme::TEXT);
    let size = Vec2::new(galley.size().x + 22.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let (fill, stroke, color) = frame_colors(&resp, enabled, selected);
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(theme::RADIUS), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Главная кнопка экрана: жёлтая.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), bold(14.0), theme::INK);
    let size = Vec2::new(galley.size().x + 28.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let fill = if !enabled {
        theme::RAISED
    } else if resp.hovered() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, if enabled { theme::INK } else { theme::TEXT_FAINT });
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// Сегментный переключатель: несколько вариантов, выбран один. Возвращает true при смене.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str, &str)]) -> bool {
    let mut changed = false;
    let font = ui_font(14.0);
    let widths: Vec<f32> =
        options.iter().map(|(_, label, _)| ui.painter().layout_no_wrap(label.to_string(), font.clone(), theme::TEXT).size().x + 24.0).collect();
    let total = widths.iter().sum::<f32>();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 28.0), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(theme::RADIUS), theme::PANEL, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    let mut x = rect.left();
    for ((v, label, hint), w) in options.iter().zip(widths) {
        let r = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w, rect.height()));
        x += w;
        let resp = ui.interact(r, ui.id().with(("seg", label)), Sense::click());
        let selected = *value == *v;
        let p = ui.painter();
        if selected {
            p.rect_filled(r.shrink(2.0), CornerRadius::same(theme::RADIUS - 1), theme::HOVER);
            p.hline(r.shrink2(Vec2::new(8.0, 0.0)).x_range(), r.bottom() - 3.0, Stroke::new(2.0, theme::ACCENT));
        } else if resp.hovered() {
            p.rect_filled(r.shrink(2.0), CornerRadius::same(theme::RADIUS - 1), theme::RAISED);
        }
        p.text(r.center(), Align2::CENTER_CENTER, *label, font.clone(), if selected { theme::TEXT } else { theme::TEXT_DIM });
        let resp = if hint.is_empty() { resp } else { resp.on_hover_text(*hint) };
        if resp.clicked() && !selected {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// Переключатель с флажком: «[x] подпись».
pub fn check(ui: &mut Ui, on: bool, text: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), ui_font(14.0), theme::TEXT);
    let size = Vec2::new(galley.size().x + 30.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let bx = Rect::from_center_size(Pos2::new(rect.left() + 9.0, rect.center().y), Vec2::splat(15.0));
    if on {
        p.rect_filled(bx, CornerRadius::same(3), theme::ACCENT);
        p.add(Shape::line(
            vec![bx.left_center() + Vec2::new(3.0, 0.5), bx.center_bottom() + Vec2::new(-1.0, -4.0), bx.right_top() + Vec2::new(-3.0, 4.0)],
            Stroke::new(2.0, theme::INK),
        ));
    } else {
        p.rect_stroke(bx, CornerRadius::same(3), Stroke::new(1.5, if resp.hovered() { theme::TEXT_DIM } else { theme::LINE_STRONG }), StrokeKind::Inside);
    }
    let color = if on || resp.hovered() { theme::TEXT } else { theme::TEXT_DIM };
    p.galley_with_override_text_color(Pos2::new(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0), galley, color);
    resp
}

/// Цветная плашка статуса с текстом.
pub fn status_pill(p: &Painter, pos: Pos2, anchor: Align2, status: Status) -> Rect {
    pill(p, pos, anchor, i18n::status_label(status), theme::status_color(status))
}

pub fn pill(p: &Painter, pos: Pos2, anchor: Align2, text: &str, color: Color32) -> Rect {
    let galley = p.layout_no_wrap(text.to_string(), bold(12.0), theme::INK);
    let rect = anchor.anchor_size(pos, galley.size() + Vec2::new(12.0, 4.0));
    p.rect_filled(rect, CornerRadius::same(theme::RADIUS), color);
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, theme::INK);
    rect
}

/// Заголовок небольшого блока.
pub fn caption(ui: &mut Ui, text: &str) -> Response {
    ui.label(RichText::new(text).font(ui_font(13.0)).color(theme::TEXT_DIM))
}
