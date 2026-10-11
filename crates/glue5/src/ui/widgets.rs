//! Shared rows and controls, sized from the space available rather than the platform.
use super::theme::{INK, MUTED};
use egui::*;

pub(super) fn focus(r: Response) -> Response {
    if r.gained_focus() {
        r.scroll_to_me(None);
    }
    r
}

pub(super) fn close_button(ui: &mut Ui) -> Response {
    let (r, response) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Close settings"));
    let c = r.center();
    ui.painter().circle_filled(c, 18.0, Color32::from_gray(if response.hovered() { 60 } else { 32 }));
    for sign in [-1.0, 1.0] {
        ui.painter()
            .line_segment([c + vec2(-5.0, -5.0 * sign), c + vec2(5.0, 5.0 * sign)], Stroke::new(1.6, Color32::WHITE));
    }
    if response.has_focus() {
        ui.painter().circle_stroke(c, 20.0, Stroke::new(1.0, Color32::WHITE));
    }
    focus(response).on_hover_text("Close")
}

#[derive(Clone, Copy)]
pub(super) enum Control {
    Slider,
    Choice,
    Toggle,
    Action,
}

pub(super) fn setting<R>(
    ui: &mut Ui,
    label: &str,
    kind: Control,
    subtitle: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.push_id(label, |ui| {
        Frame::new()
            .fill(Color32::from_gray(28))
            .corner_radius(14)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let available = ui.available_width();
                let width = match kind {
                    Control::Slider => (available * 0.55).min(260.0),
                    Control::Choice => (available * 0.60).min(180.0),
                    Control::Toggle => 44.0,
                    Control::Action => (available * 0.55).min(180.0),
                };
                let label_width = (available - width - ui.spacing().item_spacing.x).max(1.0);
                ui.horizontal_top(|ui| {
                    let (rect, response) = ui.allocate_exact_size(vec2(label_width, 44.0), Sense::hover());
                    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), label));
                    paint_label(ui, rect, label, 14.0, if subtitle.is_some() { -8.0 } else { 0.0 }, Color32::WHITE);
                    if let Some(text) = subtitle {
                        paint_label(ui, rect, text, 11.0, 10.0, MUTED);
                        response.on_hover_text(text);
                    } else {
                        response.on_hover_text(label);
                    }
                    ui.allocate_ui_with_layout(vec2(width, 44.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().slider_width = (width - 56.0).max(24.0);
                        ui.spacing_mut().interact_size.y = 44.0;
                        add(ui)
                    })
                    .inner
                })
                .inner
            })
            .inner
    })
    .inner
}

fn paint_label(ui: &Ui, rect: Rect, text: &str, size: f32, offset: f32, color: Color32) {
    let galley = WidgetText::from(RichText::new(text).size(size).color(color)).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        rect.width(),
        FontId::proportional(size),
    );
    let pos = pos2(rect.left(), rect.center().y + offset - galley.size().y * 0.5);
    ui.painter().galley(pos, galley, color);
}

pub(super) fn action_button(ui: &mut Ui, text: &str) -> Response {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        focus(ui.add(Button::new(RichText::new(text).size(13.0)).corner_radius(10).min_size(vec2(0.0, 40.0))))
    })
    .inner
}

pub(super) fn choice<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> Response {
    let previous = *value;
    let index = options.iter().position(|(v, _)| v == value).unwrap_or(0);
    let width = ui.available_width();
    let mut response = Frame::new()
        .fill(Color32::from_gray(36))
        .corner_radius(12)
        .inner_margin(3)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().interact_size.y = 38.0;
                let arrow =
                    |text| Button::new(text).fill(Color32::TRANSPARENT).corner_radius(10).min_size(vec2(38.0, 38.0));
                let left = focus(ui.add(arrow("‹"))).on_hover_text("Previous value");
                ui.allocate_ui_with_layout(
                    vec2((width - 86.0).max(12.0), 38.0),
                    Layout::centered_and_justified(Direction::LeftToRight),
                    |ui| {
                        ui.add(Label::new(RichText::new(options[index].1).size(13.0)).truncate());
                    },
                );
                let right = focus(ui.add(arrow("›"))).on_hover_text("Next value");
                if left.clicked() {
                    *value = options[(index + options.len() - 1) % options.len()].0;
                }
                if right.clicked() {
                    *value = options[(index + 1) % options.len()].0;
                }
                left | right
            })
            .inner
        })
        .inner;
    if *value != previous {
        response.mark_changed();
    }
    response
}

pub(super) fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    suffix: &str,
    decimals: usize,
) -> Response {
    ui.horizontal(|ui| {
        let start = *range.start();
        let end = *range.end();
        let (r, mut response) = ui.allocate_exact_size(vec2(ui.spacing().slider_width, 44.0), Sense::click_and_drag());
        let old = *value;
        if response.clicked() {
            response.request_focus();
        }
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - r.left() - 5.0) / (r.width() - 10.0).max(1.0)).clamp(0.0, 1.0);
            *value = ((start + t * (end - start)) / step as f32).round() * step as f32;
        }
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(response.id, EventFilter { horizontal_arrows: true, ..Default::default() })
            });
            let delta = ui.input(|i| i.num_presses(Key::ArrowRight) as f32 - i.num_presses(Key::ArrowLeft) as f32);
            *value += delta * step as f32;
        }
        *value = value.clamp(start, end);
        if *value != old {
            response.mark_changed();
        }
        response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), *value as f64, ""));
        let rail =
            Rect::from_min_max(pos2(r.left() + 5.0, r.center().y - 2.0), pos2(r.right() - 5.0, r.center().y + 2.0));
        let x = rail.left() + rail.width() * ((*value - start) / (end - start));
        ui.painter().rect_filled(rail, 1, Color32::from_gray(68));
        ui.painter().rect_filled(Rect::from_min_max(rail.min, pos2(x, rail.bottom())), 1, Color32::WHITE);
        ui.painter().circle_filled(pos2(x, r.center().y), 5.0, Color32::WHITE);
        if response.has_focus() {
            ui.painter().rect_stroke(r, 8, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
        }
        let (r, _) = ui.allocate_exact_size(vec2(46.0, 44.0), Sense::hover());
        let shown = if suffix == "%" { *value * 100.0 } else { *value };
        ui.painter().text(
            r.right_center(),
            Align2::RIGHT_CENTER,
            format!("{:.*}{}", decimals, shown, suffix),
            FontId::proportional(14.0),
            Color32::WHITE,
        );
        focus(response)
    })
    .inner
}

pub(super) fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (r, mut response) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, ""));
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    ui.painter().rect_filled(
        Rect::from_center_size(r.center(), vec2(40.0, 24.0)),
        13,
        if *on { Color32::WHITE } else { Color32::from_gray(65) },
    );
    ui.painter().circle_filled(
        pos2(egui::lerp((r.left() + 14.0)..=(r.right() - 14.0), t), r.center().y),
        8.0,
        if *on { INK } else { MUTED },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(r, 14, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
    }
    focus(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_keyboard_keeps_focus_and_reports_changes() {
        let ctx = Context::default();
        let mut value = 0.75;
        let mut id = Id::NULL;
        let mut changed = false;
        for frame in 0..3 {
            let events = if frame == 2 {
                vec![Event::Key {
                    key: Key::ArrowRight,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }]
            } else {
                vec![]
            };
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(390.0, 844.0))),
                    events,
                    ..Default::default()
                },
                |root| {
                    CentralPanel::default().show(root, |ui| {
                        let response = slider(ui, &mut value, 0.5..=1.0, 0.05, "%", 0);
                        id = response.id;
                        if frame < 2 {
                            response.request_focus();
                        }
                        changed = response.changed();
                    });
                },
            );
            output.textures_delta.clear();
        }
        assert!((value - 0.8).abs() < 0.001);
        assert!(changed);
        assert_eq!(ctx.memory(|m| m.focused()), Some(id));
    }

    #[test]
    fn controls_and_action_buttons_share_the_right_edge() {
        for width in [248.0, 318.0, 640.0] {
            let ctx = Context::default();
            let mut rect = Rect::NOTHING;
            let mut right = 0.0;
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width + 40.0, 500.0))),
                    ..Default::default()
                },
                |root| {
                    CentralPanel::default().show(root, |ui| {
                        ui.set_width(width);
                        right = ui.max_rect().right() - 12.0;
                        setting(
                            ui,
                            "Game folder",
                            Control::Action,
                            Some("A very long folder path to truncate"),
                            |ui| {
                                rect = action_button(ui, "Choose…").rect;
                            },
                        );
                        assert!((rect.right() - right).abs() < 1.0, "{width}: {rect:?}, {:?}", ui.min_rect());
                    });
                },
            );
            output.textures_delta.clear();
        }
    }
}
