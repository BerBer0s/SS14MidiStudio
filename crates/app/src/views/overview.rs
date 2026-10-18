//! Обзорная полоса: нагрузка (событий в секунду) по всей песне относительно лимитов SS14.
//! Клик и перетаскивание — показать это место в пианоролле, Shift+клик — слушать отсюда.

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use ss14midi_core::analysis::TimeRange;
use ss14midi_core::Limits;

use crate::i18n::tr;
use crate::theme::{self, ui_font};

pub struct OverviewInput<'a> {
    /// События по 100 мс — текущая рабочая копия.
    pub timeline: &'a [u32],
    /// То же для исходного файла (тень «было»).
    pub source: Option<&'a [u32]>,
    pub ranges: &'a [TimeRange],
    pub duration_s: f64,
    pub limits: &'a Limits,
    /// Видимая область пианоролла, секунды.
    pub view: (f64, f64),
    pub playhead_s: Option<f64>,
}

pub enum OverviewAction {
    Navigate(f64),
    Seek(f64),
}

/// Плотность «в скользящей секунде»: сумма 10 отрезков по 100 мс.
fn per_second(timeline: &[u32]) -> Vec<u32> {
    let mut out = vec![0u32; timeline.len()];
    let mut acc: u32 = timeline.iter().take(10).sum();
    for i in 0..timeline.len() {
        out[i] = acc;
        acc -= timeline[i];
        if let Some(v) = timeline.get(i + 10) {
            acc += v;
        }
    }
    out
}

pub fn show(ui: &mut Ui, input: &OverviewInput, height: f32) -> Option<OverviewAction> {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect(rect, egui::CornerRadius::same(theme::RADIUS), theme::PANEL, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);

    let plot = Rect::from_min_max(rect.min + Vec2::new(6.0, 16.0), rect.max - Vec2::new(6.0, 15.0));
    let duration = input.duration_s.max(0.5);
    let series = per_second(input.timeline);
    let source = input.source.map(per_second);
    let peak = series.iter().chain(source.iter().flatten()).copied().max().unwrap_or(0);
    let scale_max = (peak as f32).max(input.limits.max_per_second as f32 * 1.15);
    let y_of = |v: f32| plot.bottom() - (v / scale_max).min(1.0) * plot.height();
    let x_of = |s: f64| plot.left() + (s / duration) as f32 * plot.width();
    let bucket_s = 0.1;

    // Перегрузки — красная заливка и полоса по низу.
    for r in input.ranges {
        let x0 = x_of(r.from_s);
        let x1 = x_of(r.to_s).max(x0 + 2.0);
        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, plot.y_range()), 0.0, theme::OVER.gamma_multiply(0.12));
        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, (plot.bottom() - 3.0)..=plot.bottom()), 0.0, theme::OVER);
    }

    // Столбцы по пикселям: максимум в колонке.
    let columns = plot.width().max(1.0) as usize;
    let column_value = |data: &[u32], c: usize| -> u32 {
        let a = (c as f64 / columns as f64 * duration / bucket_s) as usize;
        let b = (((c + 1) as f64 / columns as f64 * duration / bucket_s) as usize).max(a + 1);
        data.get(a..b.min(data.len())).and_then(|s| s.iter().copied().max()).unwrap_or(0)
    };
    for c in 0..columns {
        let v = column_value(&series, c);
        if v == 0 {
            continue;
        }
        let color = if v > input.limits.max_per_second {
            theme::OVER
        } else if v > input.limits.target_per_second {
            theme::WARN
        } else {
            Color32::from_rgb(0x6B, 0x74, 0x7E)
        };
        let x = plot.left() + c as f32;
        painter.vline(x + 0.5, y_of(v as f32)..=plot.bottom(), Stroke::new(1.0, color));
    }
    if let Some(src) = &source {
        let mut pts = Vec::with_capacity(columns);
        for c in 0..columns {
            pts.push(Pos2::new(plot.left() + c as f32 + 0.5, y_of(column_value(src, c) as f32)));
        }
        painter.add(egui::Shape::line(pts, Stroke::new(1.0, theme::TEXT_FAINT)));
    }

    // Линии лимитов; подписи к ним — в строке над графиком.
    for (value, color) in [(input.limits.target_per_second, theme::ACCENT), (input.limits.max_per_second, theme::OVER)] {
        let y = y_of(value as f32);
        let mut x = plot.left();
        while x < plot.right() {
            painter.hline(x..=(x + 6.0).min(plot.right()), y, Stroke::new(1.0, color.gamma_multiply(0.8)));
            x += 10.0;
        }
    }
    let mut x = rect.left() + 8.0;
    let y = rect.top() + 2.0;
    for (i, (text, color)) in [
        (tr("Нагрузка, событий в секунду", "Load, events per second").to_string(), theme::TEXT_DIM),
        (format!("{} {}", tr("безопасно до", "safe up to"), input.limits.target_per_second), theme::ACCENT),
        (format!("{} {}", tr("лимит", "limit"), input.limits.max_per_second), theme::OVER),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            x += 14.0;
            for dx in [0.0, 8.0] {
                painter.hline((x + dx)..=(x + dx + 5.0), y + 7.0, Stroke::new(1.5, color));
            }
            x += 18.0;
        }
        let galley = painter.layout_no_wrap(text, ui_font(11.5), color);
        let w = galley.size().x;
        painter.galley(Pos2::new(x, y), galley, color);
        x += w;
    }

    // Видимая область пианоролла.
    let (v0, v1) = input.view;
    let vx0 = x_of(v0).clamp(plot.left(), plot.right());
    let vx1 = x_of(v1).clamp(plot.left(), plot.right()).max(vx0 + 3.0);
    let view_rect = Rect::from_x_y_ranges(vx0..=vx1, plot.y_range());
    painter.rect_filled(view_rect, 0.0, theme::ACCENT.gamma_multiply(0.06));
    painter.rect_stroke(view_rect, 0.0, Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7)), StrokeKind::Inside);

    if let Some(p) = input.playhead_s {
        let x = x_of(p);
        painter.vline(x, plot.y_range(), Stroke::new(1.5, theme::ACCENT));
    }

    // Шкала времени: метки раз в N секунд.
    let step = [5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0]
        .into_iter()
        .find(|s| (*s / duration) as f32 * plot.width() >= 60.0)
        .unwrap_or(600.0);
    let mut t = 0.0;
    while t <= duration {
        let x = x_of(t);
        painter.vline(x, plot.bottom()..=(plot.bottom() + 4.0), Stroke::new(1.0, theme::TEXT_FAINT));
        painter.text(Pos2::new(x + 3.0, plot.bottom() + 1.0), Align2::LEFT_TOP, fmt_time(t), ui_font(10.5), theme::TEXT_FAINT);
        t += step;
    }

    let to_s = |pos: Pos2| ((pos.x - plot.left()) / plot.width()).clamp(0.0, 1.0) as f64 * duration;
    if let Some(pos) = resp.interact_pointer_pos()
        && (resp.clicked() || resp.dragged())
    {
        let shift = ui.input(|i| i.modifiers.shift);
        return Some(if shift { OverviewAction::Seek(to_s(pos)) } else { OverviewAction::Navigate(to_s(pos)) });
    }
    if let Some(pos) = resp.hover_pos() {
        let v = series.get((to_s(pos) / bucket_s) as usize).copied().unwrap_or(0);
        resp.on_hover_text_at_pointer(format!("{} · {v} {}", fmt_time(to_s(pos)), tr("событий в секунду", "events per second")));
    }
    None
}

pub fn fmt_time(s: f64) -> String {
    let s = s.max(0.0);
    format!("{}:{:02}", (s / 60.0) as u64, (s % 60.0) as u64)
}

pub fn fmt_time_ms(s: f64) -> String {
    let s = s.max(0.0);
    format!("{}:{:02}.{}", (s / 60.0) as u64, (s % 60.0) as u64, ((s * 10.0) % 10.0) as u64)
}
