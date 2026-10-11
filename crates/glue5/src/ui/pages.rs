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
    let safe = ctx.content_rect();
    let scale = theme::layout_scale(safe);
    let width = (safe.width() - 32.0 * scale).clamp(1.0, 1100.0).floor();
    let top = (safe.height() * 0.1).clamp(12.0 * scale, 90.0 * scale).ceil();
    let height = (safe.height() - top).max(1.0).floor();
    let gap = 40.0 * scale;
    let menu_width = (360.0 * scale).min((width - gap) * 0.42);
    let status_width = (width - menu_width - gap).max(1.0);
    let mut play = false;
    egui::Area::new(Id::new("start"))
        .default_size(vec2(width, height))
        .constrain(false)
        .fixed_pos(pos2(safe.center().x - width * 0.5, safe.top() + top))
        .show(ctx, |ui| {
            theme::apply_density(ui, scale);
            ui.set_width(width);
            ui.set_height(height);
            ui.label(theme::heading("GLUE 5", 48.0 * scale));
            ui.add_space(20.0 * scale);
            let remaining = (ui.max_rect().bottom() - ui.cursor().top()).max(1.0).floor();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui_with_layout(
                    vec2(menu_width, remaining),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(menu_width);
                        egui::ScrollArea::vertical()
                            .content_margin(0)
                            .id_salt("home-menu")
                            .max_height(remaining)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0 * scale;
                                let busy = starting || status.preparing.is_some();
                                ui.add_enabled_ui(status.game_dir.is_some() && !busy, |ui| {
                                    let label = match status.preparing {
                                        Some(done) => format!("PREPARING {:.0}%", done * 100.0),
                                        None if starting => "STARTING…".into(),
                                        None => "PLAY".into(),
                                    };
                                    play = menu_button(ui, menu_width, &label, true).clicked();
                                });
                                if status.preparing.is_some() {
                                    ui.label(
                                        RichText::new(
                                            "Compiling the game's shaders for this device: minutes the first time, \
                                             seconds after.",
                                        )
                                        .small()
                                        .color(theme::MUTED),
                                    );
                                }
                                if menu_button(ui, menu_width, "SETTINGS", false).clicked() {
                                    *go = Some(Some(Page::Settings));
                                }
                                if menu_button(ui, menu_width, if MOBILE { "FIND FILES" } else { "GAME FOLDER" }, false)
                                    .clicked()
                                {
                                    actions.push(if MOBILE { Action::FindGame } else { Action::ChooseGame });
                                }
                                if !MOBILE && menu_button(ui, menu_width, "QUIT", false).clicked() {
                                    actions.push(Action::Quit);
                                }
                            });
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(status_width, remaining),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(status_width);
                        egui::ScrollArea::vertical()
                            .content_margin(0)
                            .id_salt("home-status")
                            .max_height(remaining)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                card(ui, |ui| game_files(ui, status));
                                if let Some(why) = status.last_failure {
                                    card(ui, |ui| last_failure(ui, why, status.graphics_reset, actions));
                                }
                                ui.label(RichText::new(open_hint()).small().color(theme::MUTED));
                            });
                    },
                );
            });
        });
    play
}

fn menu_button(ui: &mut egui::Ui, width: f32, label: &str, primary: bool) -> egui::Response {
    let scale = theme::density(ui);
    focus(
        ui.add_sized(
            [width, 58.0 * scale],
            egui::Button::new(theme::heading(label, 22.0 * scale).color(if primary {
                theme::INK
            } else {
                Color32::WHITE
            }))
            .fill(if primary { Color32::WHITE } else { Color32::from_black_alpha(215) })
            .corner_radius((18.0 * scale) as u8),
        ),
    )
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let scale = theme::density(ui);
    egui::Frame::new()
        .fill(Color32::from_black_alpha(210))
        .corner_radius((18.0 * scale) as u8)
        .inner_margin((18.0 * scale) as i8)
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
                .inner_margin(egui::Margin::symmetric(
                    (10.0 * theme::density(ui)) as i8,
                    (4.0 * theme::density(ui)) as i8,
                ))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(if status.game_dir.is_some() { "Ready" } else { "Missing" })
                            .size(11.0 * theme::density(ui)),
                    );
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
    let scale = theme::layout_scale(area);
    let edge = 6.0 * scale;
    let padding = (20.0 * scale) as i8;
    let size = vec2(
        (area.width() - 2.0 * edge - 2.0).clamp(1.0, 718.0),
        (area.height() - 2.0 * edge - 2.0).clamp(1.0, if scale < 1.0 { f32::INFINITY } else { 558.0 }),
    );
    // Padding lives in the scroll content, so rows scroll to the window edge.
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(0).corner_radius((22.0 * scale) as u8);
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
            theme::apply_density(ui, scale);
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: padding,
                    right: padding,
                    top: (12.0 * scale) as i8,
                    bottom: (8.0 * scale) as i8,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(theme::heading("SETTINGS", 28.0 * scale));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close = close_button(ui).clicked();
                        });
                    });
                });
            let height = (size.y - ui.min_rect().height()).max(1.0);
            egui::ScrollArea::vertical()
                .content_margin(0)
                .id_salt("settings-scroll")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0 * scale;
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: padding,
                            right: padding,
                            top: (4.0 * scale) as i8,
                            bottom: (12.0 * scale) as i8,
                        })
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .fill(Color32::from_gray(24))
                                .corner_radius((16.0 * scale) as u8)
                                .inner_margin((4.0 * scale) as i8)
                                .show(ui, |ui| {
                                    let w = (ui.available_width() - 8.0 * scale) / 3.0;
                                    ui.spacing_mut().interact_size.y = 40.0 * scale;
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0 * scale;
                                        for (value, label) in [
                                            (SettingsTab::Video, "VIDEO"),
                                            (SettingsTab::Controls, "CONTROLS"),
                                            (SettingsTab::Game, "GAME"),
                                        ] {
                                            let active = *tab == value;
                                            if focus(
                                                ui.add_sized(
                                                    [w, 40.0 * scale],
                                                    egui::Button::new(
                                                        theme::heading(label, 16.0 * scale).color(if active {
                                                            theme::INK
                                                        } else {
                                                            theme::MUTED
                                                        }),
                                                    )
                                                    .fill(if active { Color32::WHITE } else { Color32::TRANSPARENT })
                                                    .corner_radius((12.0 * scale) as u8),
                                                ),
                                            )
                                            .clicked()
                                            {
                                                *tab = value;
                                            }
                                        }
                                    });
                                });
                            ui.add_space(10.0 * scale);
                            if let Some(note) = note {
                                ui.label(RichText::new(note).small().color(WARN));
                            }
                            settings_items(ui, settings, status, *tab, scale_edit, actions);
                        });
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
            #[cfg(target_vendor = "apple")]
            setting(
                ui,
                "Metal HUD",
                Control::Toggle,
                (settings.metal_hud && !status.metal_hud).then_some("Next launch"),
                |ui| toggle(ui, &mut settings.metal_hud),
            );
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
            metal_hud: false,
            refresh_rate: 120.0,
            preparing: None,
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
    fn phone_settings_use_the_height_and_keep_video_options_reachable() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut settings = Settings::default();
        let mut tab = SettingsTab::Video;
        for pass in 0..8 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(874.0, 402.0))),
                    time: Some(pass as f64),
                    events: if pass == 4 {
                        vec![
                            egui::Event::PointerMoved(Pos2::new(430.0, 300.0)),
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: vec2(0.0, -150.0),
                                phase: egui::TouchPhase::Move,
                                modifiers: egui::Modifiers::default(),
                            },
                        ]
                    } else {
                        Vec::new()
                    },
                    safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                        left: 62.0,
                        right: 62.0,
                        top: 0.0,
                        bottom: 21.0,
                    })),
                    ..Default::default()
                },
                |root| {
                    settings_page(root.ctx(), &mut settings, &status(), None, &mut tab, &mut None, &mut Vec::new());
                },
            );
            output.textures_delta.clear();
            if pass == 3 || pass == 7 {
                let rows: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Rect(r) if r.fill == egui::Color32::from_gray(28) => Some((r.rect, s.clip_rect)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(rows.len(), if cfg!(target_vendor = "apple") { 5 } else { 4 });
                // The original options fit without scrolling. On Apple, the
                // additional HUD row must be fully reachable by scrolling.
                let visible = if pass == 3 { &rows[..4] } else { &rows[rows.len() - 1..] };
                for (rect, clip) in visible {
                    assert!(clip.contains_rect(*rect), "clipped option: {rect:?} in {clip:?}");
                }
            }
        }
        let window = ctx.memory(|m| m.area_rect(Id::new("settings-window"))).unwrap();
        let safe = ctx.content_rect();
        assert!(safe.contains_rect(window));
        assert!(safe.height() - window.height() <= 12.0, "wasted height: {window:?} in {safe:?}");
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
