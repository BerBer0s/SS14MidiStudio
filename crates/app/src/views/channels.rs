//! Каналы песни: инструмент, «без звука» и «соло» при прослушивании, видимость в пианоролле,
//! канал для рисования.

use egui::{Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use ss14midi_core::Song;
use ss14midi_core::gm::{GM_NAMES, note_name};
use ss14midi_core::model::DRUM_CHANNEL;

use crate::i18n::{count, tr};
use crate::theme::{self, bold, mono, ui_font};
use crate::widgets::{self, Icon};

pub enum ChannelAction {
    ToggleMute(usize),
    ToggleSolo(usize),
    ToggleHidden(usize),
    ShowOnly(usize),
    ShowAll,
    SetActive(usize),
    SetProgram(usize, u8),
    Clear(usize),
}

pub struct ChannelFlags {
    pub mute: u16,
    pub solo: u16,
    pub hidden: u16,
    pub active: u8,
}

fn small_toggle(ui: &mut Ui, text: &str, on: bool, on_color: Color32, hint: &str) -> bool {
    let galley = ui.painter().layout_no_wrap(text.to_string(), ui_font(12.5), theme::TEXT);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 16.0, 24.0), Sense::click());
    let p = ui.painter();
    let r = CornerRadius::same(theme::RADIUS);
    if on {
        p.rect_filled(rect, r, on_color);
    } else {
        p.rect(rect, r, if resp.hovered() { theme::HOVER } else { theme::RAISED }, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    }
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, if on { theme::INK } else { theme::TEXT_DIM });
    resp.on_hover_text(hint).clicked()
}

fn eye_button(ui: &mut Ui, visible: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::HOVER);
    }
    let color = if visible { theme::TEXT_DIM } else { theme::TEXT_FAINT };
    let icon = rect.shrink(5.0);
    widgets::paint_icon(p, icon, Icon::Eye, color);
    if !visible {
        p.line_segment([icon.left_bottom(), icon.right_top()], Stroke::new(1.6, theme::OVER));
    }
    resp.on_hover_text(if visible {
        tr("Видно в пианоролле. Клик — скрыть", "Visible in the piano roll. Click to hide")
    } else {
        tr("Скрыто в пианоролле. Клик — показать", "Hidden in the piano roll. Click to show")
    })
}

pub fn show(ui: &mut Ui, song: &Song, flags: &ChannelFlags) -> Vec<ChannelAction> {
    let mut actions = Vec::new();
    let armed_id = ui.id().with("clear-armed");
    let armed: Option<usize> = ui.data(|d| d.get_temp(armed_id));
    let used: Vec<usize> = (0..16).filter(|&c| !song.channels[c].is_empty() || c == flags.active as usize).collect();

    if flags.hidden & used.iter().fold(0u16, |m, &c| m | 1 << c) != 0 {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Часть каналов скрыта", "Some channels are hidden")).font(ui_font(12.5)).color(theme::TEXT_DIM));
            if ui.link(RichText::new(tr("показать все", "show all")).font(ui_font(12.5))).clicked() {
                actions.push(ChannelAction::ShowAll);
            }
        });
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for ch in used {
            let c = &song.channels[ch];
            let color = theme::channel_color(ch);
            let active = flags.active as usize == ch;
            let hidden = flags.hidden & (1 << ch) != 0;
            // Карточка целиком кликабельна; кнопки внутри получают клики первыми.
            let builder = egui::UiBuilder::new().id_salt(("card", ch)).sense(Sense::click());
            let resp = ui
                .scope_builder(builder, |ui| {
                    egui::Frame::NONE
                        .fill(if active { theme::RAISED } else { theme::PANEL })
                        .stroke(Stroke::new(1.0, if active { theme::ACCENT.gamma_multiply(0.7) } else { theme::LINE }))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (sw, _) = ui.allocate_exact_size(Vec2::new(12.0, 12.0), Sense::hover());
                                ui.painter().rect_filled(sw, CornerRadius::same(3), if hidden { color.gamma_multiply(0.3) } else { color });
                                ui.label(RichText::new(format!("{}", ch + 1)).font(mono(12.5)).color(theme::TEXT_DIM));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if eye_button(ui, !hidden).clicked() {
                                        actions.push(ChannelAction::ToggleHidden(ch));
                                    }
                                    let name = song.channel_label(ch);
                                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                        ui.add(
                                            egui::Label::new(RichText::new(name).font(bold(14.0)).color(if hidden { theme::TEXT_FAINT } else { theme::TEXT }))
                                                .truncate(),
                                        );
                                    });
                                });
                            });
                            if ch == DRUM_CHANNEL {
                                ui.label(RichText::new(tr("Ударные", "Drums")).color(theme::TEXT_DIM));
                            } else {
                                let program = c.first_program().unwrap_or(0);
                                egui::ComboBox::from_id_salt(("prog", ch))
                                    .width(ui.available_width() - 4.0)
                                    .selected_text(GM_NAMES[program as usize])
                                    .height(360.0)
                                    .show_ui(ui, |ui| {
                                        for (p, name) in GM_NAMES.iter().enumerate() {
                                            if ui.selectable_label(p as u8 == program, format!("{:3}  {name}", p + 1)).clicked() && p as u8 != program {
                                                actions.push(ChannelAction::SetProgram(ch, p as u8));
                                            }
                                        }
                                    })
                                    .response
                                    .on_hover_text(tr("Инструмент канала", "Channel instrument"));
                            }
                            let range = match (c.notes.iter().map(|n| n.key).min(), c.notes.iter().map(|n| n.key).max()) {
                                (Some(a), Some(b)) => format!(" · {}–{}", note_name(a), note_name(b)),
                                _ => String::new(),
                            };
                            let mut info = count(c.notes.len(), ["нота", "ноты", "нот"], ["note", "notes"]) + &range;
                            if active {
                                info += tr(" · сюда рисуются ноты", " · new notes go here");
                            }
                            ui.label(RichText::new(info).font(ui_font(12.5)).color(if active { theme::ACCENT } else { theme::TEXT_FAINT }));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                if small_toggle(
                                    ui,
                                    tr("Без звука", "Mute"),
                                    flags.mute & (1 << ch) != 0,
                                    theme::WARN,
                                    tr("Не играть этот канал при прослушивании", "Do not play this channel during playback"),
                                ) {
                                    actions.push(ChannelAction::ToggleMute(ch));
                                }
                                if small_toggle(
                                    ui,
                                    tr("Соло", "Solo"),
                                    flags.solo & (1 << ch) != 0,
                                    theme::ACCENT,
                                    tr("Слушать только этот канал", "Play only this channel"),
                                ) {
                                    actions.push(ChannelAction::ToggleSolo(ch));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if armed == Some(ch) {
                                        if small_toggle(
                                            ui,
                                            tr("Удалить канал?", "Delete channel?"),
                                            true,
                                            theme::OVER,
                                            tr("Ещё раз — удалить все ноты и настройки канала", "Click again to delete all notes and settings of the channel"),
                                        ) {
                                            actions.push(ChannelAction::Clear(ch));
                                            ui.data_mut(|d| d.remove::<usize>(armed_id));
                                        }
                                    } else if widgets::icon_button(ui, Icon::Trash, tr("Удалить канал", "Delete channel"), true).clicked() {
                                        ui.data_mut(|d| d.insert_temp(armed_id, ch));
                                    }
                                });
                            });
                        })
                })
                .response;
            if resp.double_clicked() {
                actions.push(ChannelAction::ShowOnly(ch));
            } else if resp.clicked() && !active {
                actions.push(ChannelAction::SetActive(ch));
            }
            resp.on_hover_text(tr(
                "Клик — рисовать в этот канал, двойной клик — показать только его",
                "Click to draw into this channel, double-click to show only it",
            ));
            ui.add_space(6.0);
        }
        // Пустой канал под рисование.
        let free: Vec<usize> = (0..16).filter(|&c| song.channels[c].is_empty() && c != flags.active as usize).collect();
        if !free.is_empty() {
            egui::ComboBox::from_id_salt("new-channel")
                .selected_text(tr("+ Новый канал для рисования", "+ New channel to draw into"))
                .width(ui.available_width() - 4.0)
                .show_ui(ui, |ui| {
                    for c in free {
                        let label = if c == DRUM_CHANNEL { format!("{} · {}", c + 1, tr("ударные", "drums")) } else { format!("{}", c + 1) };
                        if ui.selectable_label(false, label).clicked() {
                            actions.push(ChannelAction::SetActive(c));
                        }
                    }
                });
        }
    });
    actions
}
