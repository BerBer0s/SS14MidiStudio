//! Отчёт: что было не так с исходным файлом, что изменила обработка, где перегрузки сейчас.
//! Клик по времени — переход к этому месту.

use std::collections::BTreeMap;

use egui::{RichText, Ui};
use ss14midi_core::gm::note_name;
use ss14midi_core::passes::limiter::Tier;

use crate::i18n::{self, count, tr};
use crate::library::Entry;
use crate::theme::{self, bold, mono};
use crate::views::overview::fmt_time_ms;

pub enum ReportAction {
    JumpSeconds(f64),
    Restore(usize),
    RestoreAll,
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).font(bold(14.0)).color(theme::TEXT));
    ui.add_space(2.0);
}

fn line(ui: &mut Ui, label: &str, value: String) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).font(mono(13.0)).color(theme::TEXT));
        });
    });
}

fn text(ui: &mut Ui, s: &str, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(s).color(color)).wrap());
}

fn tier_label(t: Tier) -> &'static str {
    match t {
        Tier::A => tr("дубли в унисон", "unison duplicates"),
        Tier::B => tr("микроноты", "micro notes"),
        Tier::C => tr("октавные удвоения", "octave doublings"),
        Tier::D => tr("внутренние голоса", "inner voices"),
        Tier::E => tr("ударные", "drums"),
    }
}

pub fn show(ui: &mut Ui, entry: &Entry) -> Vec<ReportAction> {
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if let Some(src) = &entry.source {
            heading(ui, tr("Исходный файл", "Source file"));
            let m = &src.analysis.metrics;
            line(ui, tr("Длительность", "Duration"), fmt_time_ms(m.duration_s));
            line(ui, tr("Пик событий в секунду", "Peak events per second"), m.peak_per_second.value.to_string());
            line(ui, tr("Пик событий за кадр", "Peak events per frame"), m.peak_per_frame.value.to_string());
            line(ui, tr("Всего событий", "Total events"), m.events_total.to_string());
            let i = &src.issues;
            let fixed = [
                (i.orphan_offs, tr("Лишних note_off", "Stray note-offs")),
                (i.overlaps_resolved + i.same_start_merged, tr("Наложений нот", "Overlapping notes")),
                (i.hanging_closed, tr("Незакрытых нот", "Unclosed notes")),
                (i.zero_length_extended, tr("Нот нулевой длины", "Zero-length notes")),
            ];
            if fixed.iter().any(|f| f.0 > 0) {
                heading(ui, tr("Исправлено при открытии", "Fixed on open"));
                for (n, label) in fixed.into_iter().filter(|f| f.0 > 0) {
                    line(ui, label, n.to_string());
                }
            }
            if !src.problems.is_empty() {
                heading(ui, tr("Повреждения файла", "File damage"));
                for p in &src.problems {
                    text(ui, &format!("• {p}"), theme::WARN);
                }
                text(ui, tr("Всё, что удалось прочитать, сохранено.", "Everything readable has been kept."), theme::TEXT_FAINT);
            }
        }

        match &entry.report {
            None => {
                heading(ui, tr("Обработка", "Processing"));
                text(
                    ui,
                    tr("Файл ещё не обработан. Выберите режим и нажмите «Обработать».", "Not processed yet. Choose a mode and press “Process”."),
                    theme::TEXT_DIM,
                );
            }
            Some(r) => {
                heading(ui, format!("{} · {}", tr("Обработка", "Processing"), i18n::preset_label(r.preset)).as_str());
                line(ui, tr("Пик в секунду", "Peak per second"), format!("{} → {}", r.before.peak_per_second.value, r.after.peak_per_second.value));
                line(ui, tr("Пик за кадр", "Peak per frame"), format!("{} → {}", r.before.peak_per_frame.value, r.after.peak_per_frame.value));
                line(ui, tr("Событий", "Events"), format!("{} → {}", r.before.events_total, r.after.events_total));
                let c = &r.changes;
                if c.trim.trimmed_s > 0.01 {
                    line(ui, tr("Убрано тишины в начале", "Leading silence removed"), format!("{:.2} {}", c.trim.trimmed_s, tr("с", "s")));
                }
                if let Some(v) = &c.velocity {
                    if let Some(f) = v.drum_factor {
                        line(ui, tr("Громкость ударных", "Drum volume"), format!("×{f:.2}"));
                    }
                    if let Some((_, f)) = v.melodic_factors.first() {
                        line(ui, tr("Громкость мелодии", "Melodic volume"), format!("×{f:.2}"));
                    }
                }
                if let Some(cl) = &c.cleanup {
                    let removed = cl.unused_removed + cl.redundant_removed + cl.thinned;
                    if removed > 0 {
                        line(ui, tr("Убрано лишних событий", "Redundant events removed"), removed.to_string());
                    }
                }
                if !c.removed_notes.is_empty() {
                    heading(ui, tr("Убранные ноты", "Removed notes"));
                    let mut by_tier: BTreeMap<Tier, usize> = BTreeMap::new();
                    for n in &c.removed_notes {
                        *by_tier.entry(n.tier).or_default() += 1;
                    }
                    for (t, n) in by_tier {
                        line(ui, tier_label(t), n.to_string());
                    }
                    line(ui, tr("Доля от всех нот", "Share of all notes"), format!("{:.1}%", c.removed_share * 100.0));
                    if c.heavily_simplified {
                        text(
                            ui,
                            tr(
                                "Песня заметно упрощена: она слишком плотная для одного инструмента.",
                                "The song is noticeably simplified: it is too dense for one instrument.",
                            ),
                            theme::WARN,
                        );
                    }
                    text(
                        ui,
                        tr(
                            "В пианоролле они видны красными рамками; двойной клик по рамке возвращает ноту.",
                            "They show as red outlines in the piano roll; double-click one to bring the note back.",
                        ),
                        theme::TEXT_FAINT,
                    );
                    if !entry.ghosts.is_empty() && ui.button(tr("Вернуть все", "Restore all")).clicked() {
                        actions.push(ReportAction::RestoreAll);
                    }
                    ui.add_space(4.0);
                    egui::Grid::new("removed").num_columns(3).spacing([10.0, 2.0]).show(ui, |ui| {
                        for (i, g) in entry.ghosts.iter().enumerate().take(300) {
                            if ui.link(RichText::new(fmt_time_ms(g.at_s)).font(mono(12.0))).clicked() {
                                actions.push(ReportAction::JumpSeconds(g.at_s));
                            }
                            ui.label(
                                RichText::new(format!("{} {} · {}", tr("кан.", "ch"), g.channel, note_name(g.key)))
                                    .font(mono(12.0))
                                    .color(theme::channel_color(g.channel as usize - 1)),
                            )
                            .on_hover_text(tier_label(g.tier));
                            if ui.small_button(tr("вернуть", "restore")).clicked() {
                                actions.push(ReportAction::Restore(i));
                            }
                            ui.end_row();
                        }
                    });
                    if entry.ghosts.len() > 300 {
                        text(
                            ui,
                            &format!("… {} {}", tr("и ещё", "and"), count(entry.ghosts.len() - 300, ["нота", "ноты", "нот"], ["more note", "more notes"])),
                            theme::TEXT_FAINT,
                        );
                    }
                }
            }
        }

        let ranges = entry.analysis.as_ref().map(|a| a.ranges.as_slice()).unwrap_or_default();
        if !ranges.is_empty() {
            heading(ui, tr("Перегруженные места сейчас", "Overloaded spots now"));
            for range in ranges.iter().take(100) {
                ui.horizontal(|ui| {
                    if ui.link(RichText::new(format!("{} – {}", fmt_time_ms(range.from_s), fmt_time_ms(range.to_s))).font(mono(12.0))).clicked() {
                        actions.push(ReportAction::JumpSeconds(range.from_s));
                    }
                    ui.label(RichText::new(format!("{} {}", tr("пик", "peak"), range.peak)).font(mono(12.0)).color(theme::OVER));
                });
            }
        }
    });
    actions
}
