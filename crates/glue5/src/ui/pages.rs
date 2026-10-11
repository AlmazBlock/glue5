//! Home and the floating settings window share the same responsive controls.
use super::widgets::{Control, action_button, choice, close_button, focus, setting, slider, toggle};
use super::{Action, Page, SettingsTab, Status, WARN, theme};
use crate::paths::{self, MOBILE};
use crate::settings::{self, RENDER_SCALES, Settings, TouchMode};
use egui::{Align2, Color32, Id, RichText, pos2, vec2};

/// The only platform differences are capabilities, not layout.
fn open_hint() -> &'static str {
    if MOBILE { "Settings: ⚙ or Back + Start" } else { "Settings: F1 or Back + Start" }
}

pub(super) fn start_page(
    ctx: &egui::Context,
    status: &Status,
    starting: bool,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) -> bool {
    let r = ctx.content_rect();
    let width = (r.width() - 48.0).clamp(1.0, 1100.0);
    let top = (r.height() * 0.1).clamp(24.0, 90.0).min(r.height() * 0.25);
    let height = (r.height() - top - 24.0).max(1.0);
    let wide = width >= 780.0;
    let menu_width = if wide { 360.0 } else { width };
    let status_width = if wide { width - menu_width - 40.0 } else { width };
    let mut play = false;
    egui::Area::new(Id::new("start"))
        .default_size(vec2(width, height))
        .fixed_pos(pos2(r.center().x - width * 0.5, r.top() + top))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.set_height(height);
            egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
                ui.label(theme::heading("GLUE 5", 48.0));
                ui.add_space(28.0);
                egui::Grid::new("home-columns").num_columns(if wide { 2 } else { 1 }).spacing(vec2(40.0, 24.0)).show(
                    ui,
                    |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(menu_width);
                            ui.spacing_mut().item_spacing.y = 6.0;
                            let label = if starting { "STARTING…" } else { "PLAY" };
                            ui.add_enabled_ui(status.game_dir.is_some() && !starting, |ui| {
                                play = focus(
                                    ui.add_sized(
                                        [menu_width, 58.0],
                                        egui::Button::new(theme::heading(label, 22.0).color(theme::INK))
                                            .fill(Color32::WHITE)
                                            .corner_radius(18),
                                    ),
                                )
                                .clicked();
                            });
                            for (label, action) in [
                                ("SETTINGS", None),
                                (
                                    if MOBILE { "FIND FILES" } else { "GAME FOLDER" },
                                    Some(if MOBILE { Action::FindGame } else { Action::ChooseGame }),
                                ),
                            ] {
                                if focus(
                                    ui.add_sized(
                                        [menu_width, 58.0],
                                        egui::Button::new(theme::heading(label, 22.0))
                                            .fill(Color32::from_black_alpha(215))
                                            .corner_radius(18),
                                    ),
                                )
                                .clicked()
                                {
                                    if let Some(action) = action {
                                        actions.push(action);
                                    } else {
                                        *go = Some(Some(Page::Settings));
                                    }
                                }
                            }
                            if !MOBILE
                                && focus(
                                    ui.add_sized(
                                        [menu_width, 58.0],
                                        egui::Button::new(theme::heading("QUIT", 22.0))
                                            .fill(Color32::from_black_alpha(215))
                                            .corner_radius(18),
                                    ),
                                )
                                .clicked()
                            {
                                actions.push(Action::Quit);
                            }
                        });
                        if !wide {
                            ui.end_row();
                        }
                        ui.vertical(|ui| {
                            ui.set_width(status_width);
                            card(ui, |ui| game_files(ui, status));
                            if let Some(why) = status.last_failure {
                                card(ui, |ui| last_failure(ui, why, status.graphics_reset, actions));
                            }
                            ui.label(RichText::new(open_hint()).small().color(theme::MUTED));
                        });
                        ui.end_row();
                    },
                );
                ui.add_space(24.0);
                ui.label(
                    RichText::new("D-pad  Navigate   Enter / A  Select   Esc / B  Back").small().color(theme::MUTED),
                );
            });
        });
    play
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(Color32::from_black_alpha(210))
        .corner_radius(18)
        .inner_margin(18)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn game_files(ui: &mut egui::Ui, status: &Status) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 0.0;
        ui.label("Game files");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::Frame::new()
                .fill(Color32::from_gray(42))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    ui.label(RichText::new(if status.game_dir.is_some() { "Ready" } else { "Missing" }).size(11.0));
                });
        });
    });
    if let Some(dir) = status.game_dir {
        let text = dir.display().to_string();
        ui.add(egui::Label::new(RichText::new(&text).small().color(theme::MUTED)).truncate()).on_hover_text(text);
    } else {
        if let Some(problem) = status.problem {
            ui.label(RichText::new(problem).small().color(WARN));
        }
        ui.label(RichText::new(paths::where_the_game_goes()).small().color(theme::MUTED));
        ui.label(RichText::new(paths::game_layout()).monospace().small().color(theme::MUTED));
    }
}

fn graphics_reset_note() -> RichText {
    RichText::new("Graphics reset. Backup: settings.xml.bak").small().color(theme::MUTED)
}

fn last_failure(ui: &mut egui::Ui, why: &str, reset: bool, actions: &mut Vec<Action>) {
    ui.label("Last session stopped");
    ui.label(RichText::new(why).small().color(theme::MUTED));
    if reset {
        ui.label(graphics_reset_note());
    } else {
        if action_button(ui, "Reset graphics").clicked() {
            actions.push(Action::ResetGraphics);
        }
    }
}

pub(super) fn settings_page(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    note: Option<&str>,
    tab: &mut SettingsTab,
    scale_edit: &mut Option<f32>,
    actions: &mut Vec<Action>,
) -> bool {
    let area = ctx.content_rect();
    // Sizes describe the content; include padding and stroke when fitting to
    // the safe area. This remains a floating window even on narrow screens.
    let size = vec2((area.width() - 72.0).clamp(1.0, 680.0), (area.height() - 104.0).clamp(1.0, 520.0));
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(20);
    let mut close = false;
    egui::Window::new("Settings")
        .id(Id::new("settings-window"))
        .title_bar(false)
        .movable(false)
        .resizable(false)
        .collapsible(false)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(area.center())
        .fixed_size(size)
        .frame(frame)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::heading("SETTINGS", 28.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = close_button(ui).clicked();
                });
            });
            let height = (size.y - ui.min_rect().height() - ui.spacing().item_spacing.y).max(1.0);
            egui::ScrollArea::vertical()
                .id_salt("settings-scroll")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::new().fill(Color32::from_gray(24)).corner_radius(16).inner_margin(4).show(ui, |ui| {
                        let w = (ui.available_width() - 8.0) / 3.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (value, label) in [
                                (SettingsTab::Video, "VIDEO"),
                                (SettingsTab::Controls, "CONTROLS"),
                                (SettingsTab::Game, "GAME"),
                            ] {
                                let active = *tab == value;
                                if focus(
                                    ui.add_sized(
                                        [w, 40.0],
                                        egui::Button::new(theme::heading(label, 16.0).color(if active {
                                            theme::INK
                                        } else {
                                            theme::MUTED
                                        }))
                                        .fill(if active { Color32::WHITE } else { Color32::TRANSPARENT })
                                        .corner_radius(12),
                                    ),
                                )
                                .clicked()
                                {
                                    *tab = value;
                                }
                            }
                        });
                    });
                    ui.add_space(10.0);
                    if let Some(note) = note {
                        ui.label(RichText::new(note).small().color(WARN));
                    }
                    settings_items(ui, settings, status, *tab, scale_edit, actions);
                });
        });
    close
}

fn settings_items(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    status: &Status,
    tab: SettingsTab,
    scale_edit: &mut Option<f32>,
    actions: &mut Vec<Action>,
) {
    let started = status.started;
    match tab {
        SettingsTab::Video => {
            let mut scale = scale_edit.unwrap_or(settings.render_scale());
            let (rw, rh) = settings::render_size(status.window, scale, settings.low_memory);
            let subtitle = format!(
                "{rw} × {rh}{}",
                if MOBILE && started.is_some_and(|s| s.render_scale != settings.render_scale()) {
                    " · next start"
                } else {
                    ""
                }
            );
            let r = setting(ui, "Resolution", Control::Slider, Some(&subtitle), |ui| {
                slider(ui, &mut scale, RENDER_SCALES, 0.05, "%", 0)
            });
            if r.dragged() {
                *scale_edit = Some(scale);
            } else if r.changed() || r.drag_stopped() {
                settings.render_scale = Some(settings::round_scale(scale));
                *scale_edit = None;
            }
            if status.metalfx {
                setting(ui, "Upscaling", Control::Choice, None, |ui| {
                    choice(ui, &mut settings.metalfx, &[(false, "Bicubic"), (true, "MetalFX")])
                });
            }
            let unlimited = if settings.vsync { "Display" } else { "Unlimited" };
            let mut rates = vec![(0, unlimited), (60, "60")];
            if status.refresh_rate >= 119.0 {
                rates.push((40, "40"));
            }
            rates.push((30, "30"));
            setting(
                ui,
                "Frame rate",
                Control::Choice,
                next_start(started.is_some_and(|s| s.max_fps != settings.max_fps)),
                |ui| choice(ui, &mut settings.max_fps, &rates),
            );
            setting(ui, "VSync", Control::Toggle, None, |ui| toggle(ui, &mut settings.vsync));
        }
        SettingsTab::Controls => {
            setting(ui, "Touch controls", Control::Choice, None, |ui| {
                choice(
                    ui,
                    &mut settings.touch,
                    &[(TouchMode::Auto, "Auto"), (TouchMode::Always, "On"), (TouchMode::Off, "Off")],
                )
            })
            .on_hover_text("Auto: shown on touch screens; hidden while using a controller or keyboard");
            setting(ui, "Size", Control::Slider, None, |ui| {
                slider(ui, &mut settings.touch_size, 0.7..=1.5, 0.1, "×", 1)
            });
            setting(ui, "Opacity", Control::Slider, None, |ui| {
                slider(ui, &mut settings.touch_opacity, 0.2..=1.0, 0.01, "%", 0)
            });
        }
        SettingsTab::Game => {
            let r = setting(
                ui,
                "Low memory",
                Control::Toggle,
                next_start(started.is_some_and(|s| s.low_memory != settings.low_memory)),
                |ui| toggle(ui, &mut settings.low_memory),
            )
            .on_hover_text("Smaller engine pools; enabling this also lowers graphics settings once");
            if r.changed() && settings.low_memory {
                settings.low_preset_written = false;
            }
            let dir = if started.is_some() { settings.game_dir.as_deref() } else { status.game_dir };
            let mut shown = dir.map_or_else(|| "Not found".to_owned(), |d| d.display().to_string());
            if started.is_some_and(|s| Some(s.game_dir.as_path()) != settings.game_dir.as_deref()) {
                shown.push_str(" · next start");
            }
            setting(ui, "Game folder", Control::Action, Some(&shown), |ui| {
                if action_button(ui, if MOBILE { "Find files" } else { "Choose…" }).clicked() {
                    actions.push(if MOBILE { Action::FindGame } else { Action::ChooseGame });
                }
            });
            if started.is_none() {
                setting(
                    ui,
                    "Graphics",
                    Control::Action,
                    if status.graphics_reset { Some("Reset to defaults") } else { None },
                    |ui| {
                        ui.add_enabled_ui(!status.graphics_reset && status.game_dir.is_some(), |ui| {
                            if action_button(ui, "Reset graphics").clicked() {
                                actions.push(Action::ResetGraphics);
                            }
                        });
                    },
                );
            }
            if started.is_some() && !MOBILE {
                setting(ui, "Session", Control::Action, None, |ui| {
                    if action_button(ui, "Quit game").clicked() {
                        actions.push(Action::Quit);
                    }
                });
            }
        }
    }
}

fn next_start(differs: bool) -> Option<&'static str> {
    differs.then_some("Next start")
}

#[cfg(test)]
mod tests {
    use super::{SettingsTab, Status, settings_page, start_page, theme};
    use crate::settings::Settings;
    use egui::{Id, Pos2, Rect, SafeAreaInsets, vec2};
    use std::path::Path;

    fn status<'a>() -> Status<'a> {
        Status {
            game_dir: Some(Path::new("game")),
            problem: None,
            started: None,
            last_failure: None,
            graphics_reset: false,
            window: (1440, 900),
            metalfx: true,
            refresh_rate: 120.0,
        }
    }

    #[test]
    fn settings_fit_the_safe_area_at_all_sizes() {
        for size in
            [vec2(1440.0, 900.0), vec2(768.0, 1024.0), vec2(390.0, 844.0), vec2(320.0, 480.0), vec2(844.0, 390.0)]
        {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            let mut settings = Settings::default();
            for tab in [SettingsTab::Video, SettingsTab::Controls, SettingsTab::Game] {
                let mut tab = tab;
                for _ in 0..4 {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(screen),
                            safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                                top: 24.0,
                                bottom: 16.0,
                                left: 8.0,
                                right: 8.0,
                            })),
                            ..Default::default()
                        },
                        |root| {
                            let ctx = root.ctx();
                            settings_page(ctx, &mut settings, &status(), None, &mut tab, &mut None, &mut Vec::new());
                        },
                    );
                    output.textures_delta.clear();
                }
                let window = ctx.memory(|m| m.area_rect(Id::new("settings-window"))).unwrap();
                let safe = ctx.content_rect();
                assert!(safe.contains_rect(window), "{size:?}: {window:?} outside {safe:?}");
                assert!(window.center().distance(safe.center()) < 2.0, "window not centered: {window:?}, {safe:?}");
            }
        }
    }
    #[test]
    fn home_reflows_when_the_same_window_is_resized() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for size in
            [vec2(1440.0, 900.0), vec2(390.0, 844.0), vec2(320.0, 480.0), vec2(900.0, 500.0), vec2(1440.0, 900.0)]
        {
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            for _ in 0..4 {
                let mut output =
                    ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |root| {
                        start_page(root.ctx(), &status(), false, &mut Vec::new(), &mut None);
                    });
                output.textures_delta.clear();
            }
            let home = ctx.memory(|m| m.area_rect(Id::new("start"))).unwrap();
            assert!(screen.contains_rect(home), "{size:?}: {home:?}");
        }
    }
}
