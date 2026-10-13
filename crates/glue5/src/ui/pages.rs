//! Home and the floating settings window share the same responsive controls.
use super::widgets::{Control, action_button, choice, close_button, focus, setting, slider, text_field, toggle};
use super::{Action, Mode, Page, SettingsTab, Status, WARN, theme};
use crate::paths::{self, MOBILE};
use crate::settings::{self, RENDER_SCALES, Settings, TouchMode};
use egui::{Align, Align2, Color32, Id, Layout, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};

/// The only platform differences are capabilities, not layout.
fn open_hint() -> &'static str {
    if MOBILE { "Settings: ⚙ or Back + Start" } else { "Settings: F1 or Back + Start" }
}

/// The ways into the game, the start page's main column: a name and what
/// it is.
const MODES: [(Mode, &str, &str); 4] = [
    (Mode::Story, "STORY MODE", "Michael, Franklin and Trevor's story, from your saves."),
    (Mode::Online, "GTA ONLINE", "Freemode with other players, matched automatically, no servers."),
    (Mode::Sandbox, "SANDBOX", "The whole map, free roam, no missions."),
    (Mode::Benchmark, "BENCHMARK", "The game's fly-through of the map: how it runs here."),
];

/// A mode's card at least, and the title's row, in points at full density
/// (to place the page before it is laid out).
const MODE_HEIGHT: f32 = 66.0;
const TITLE_HEIGHT: f32 = 58.0;
const GAP: f32 = 8.0;

/// A status dot: the game's files are there.
const READY: Color32 = Color32::from_rgb(120, 205, 145);

/// The start page: the title with the app's own buttons, the game's modes
/// and, quieter, what the app knows. Two columns where there is room, one
/// on an upright phone. Returns the mode chosen, if one was. `starting`:
/// the mode chosen at the last frame.
pub(super) fn start_page(
    ctx: &egui::Context,
    status: &Status,
    starting: Option<Mode>,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) -> Option<Mode> {
    let safe = ctx.content_rect();
    let scale = theme::layout_scale(safe);
    let edge = 16.0 * scale;
    let available = (safe.width() - 2.0 * edge).clamp(1.0, 1100.0).floor();
    let gap = 32.0 * scale;
    // Upright screens read better in one column: a phone, a tablet.
    let two_columns = available >= 600.0 * scale && safe.width() >= safe.height();
    let width = if two_columns { available } else { available.min(520.0) };
    let menu_width = if two_columns { (420.0 * scale).min((width - gap) * 0.5).floor() } else { width };
    // A little above the middle when there is room: the title and the
    // modes (the other cards come and go), on one column all of it.
    let modes = MODES.len() as f32 * (MODE_HEIGHT + GAP) * scale;
    let content = (TITLE_HEIGHT + 20.0) * scale + modes + if two_columns { 0.0 } else { 160.0 * scale };
    let top = ((safe.height() - content) * 0.4).max(edge * 0.5).floor();
    let height = (safe.height() - top - edge * 0.5).max(1.0).floor();
    let mut chosen = None;
    egui::Area::new(Id::new("start"))
        .default_size(vec2(width, height))
        .constrain(false)
        .fixed_pos(pos2(safe.center().x - width * 0.5, safe.top() + top))
        .show(ctx, |ui| {
            theme::apply_density(ui, scale);
            ui.set_width(width);
            ui.set_height(height);
            // The buttons go in last, so that the first move of a
            // controller lands on the modes.
            let (title_row, _) = ui.allocate_exact_size(vec2(width, TITLE_HEIGHT * scale), Sense::hover());
            ui.painter().text(
                title_row.left_center(),
                Align2::LEFT_CENTER,
                "GLUE 5",
                egui::FontId::new(48.0 * scale, egui::FontFamily::Name("condensed".into())),
                Color32::WHITE,
            );
            ui.add_space(20.0 * scale);
            let remaining = (ui.max_rect().bottom() - ui.cursor().top()).max(1.0).floor();
            if two_columns {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    column(ui, "home-modes", menu_width, remaining, |ui| {
                        chosen = mode_list(ui, status, starting, menu_width);
                    });
                    column(ui, "home-status", width - menu_width - gap, remaining, |ui| info(ui, status, actions));
                });
            } else {
                column(ui, "home", width, remaining, |ui| {
                    // Without the game's files, why first.
                    if status.game_dir.is_none() {
                        info(ui, status, actions);
                        ui.add_space(8.0 * scale);
                    }
                    chosen = mode_list(ui, status, starting, width);
                    if status.game_dir.is_some() {
                        ui.add_space(8.0 * scale);
                        info(ui, status, actions);
                    }
                });
            }
            let labels = width >= 720.0 * scale;
            ui.scope_builder(UiBuilder::new().max_rect(title_row).layout(Layout::right_to_left(Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0 * scale;
                if !MOBILE && icon_button(ui, "🗙", "Quit", labels).clicked() {
                    actions.push(Action::Quit);
                }
                let files = if MOBILE { "Find files" } else { "Game folder" };
                if icon_button(ui, "🗁", files, labels).clicked() {
                    actions.push(if MOBILE { Action::FindGame } else { Action::ChooseGame });
                }
                if icon_button(ui, "★", "Credits", labels).clicked() {
                    *go = Some(Some(Page::Credits));
                }
                if icon_button(ui, "⚙", "Settings", labels).clicked() {
                    *go = Some(Some(Page::Settings));
                }
            });
        });
    chosen
}

/// A column of the start page, scrolling if it has to.
fn column(ui: &mut egui::Ui, id: &str, width: f32, height: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(10.0, GAP) * theme::density(ui);
        egui::ScrollArea::vertical()
            .content_margin(0)
            .id_salt(id)
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, add);
    });
}

/// The modes, one card each. The one on its way (chosen at the last frame,
/// or waiting for its pipelines) says so; the others wait.
fn mode_list(ui: &mut egui::Ui, status: &Status, starting: Option<Mode>, width: f32) -> Option<Mode> {
    let going = starting.or(status.preparing.map(|_| status.mode));
    let ready = status.game_dir.is_some() && going.is_none();
    let mut chosen = None;
    for (mode, name, about) in MODES {
        let primary = mode == status.mode;
        if going == Some(mode) {
            match status.preparing {
                Some(done) => mode_card(
                    ui,
                    width,
                    &format!("PREPARING {:.0}%", done * 100.0),
                    "Compiling the game's shaders for this device: minutes the first time, seconds after.",
                    primary,
                    Some(done),
                ),
                None => mode_card(ui, width, "STARTING…", about, primary, None),
            };
        } else if ui.add_enabled_ui(ready, |ui| mode_card(ui, width, name, about, primary, None)).inner.clicked() {
            chosen = Some(mode);
        }
    }
    chosen
}

/// A mode: its name and what it is, a chevron, the whole card a button.
/// The last one played is white. `progress`: a bar along the bottom.
fn mode_card(
    ui: &mut egui::Ui,
    width: f32,
    name: &str,
    about: &str,
    primary: bool,
    progress: Option<f32>,
) -> egui::Response {
    let scale = theme::density(ui);
    let pad = vec2(20.0, 12.0) * scale;
    let (ink, soft) = if primary { (theme::INK, Color32::from_gray(92)) } else { (Color32::WHITE, theme::MUTED) };
    let text_width = (width - 2.0 * pad.x - 24.0 * scale).max(1.0);
    let galley = |text: egui::RichText, wrap| {
        egui::WidgetText::from(text).into_galley(ui, Some(wrap), text_width, egui::TextStyle::Body)
    };
    let title = galley(theme::heading(name, 22.0 * scale).color(ink), egui::TextWrapMode::Truncate);
    let about = galley(RichText::new(about).size(12.0 * scale).color(soft), egui::TextWrapMode::Wrap);
    let text_height = title.size().y + 2.0 * scale + about.size().y;
    let height = (text_height + 2.0 * pad.y).max(MODE_HEIGHT * scale);
    let interactive = progress.is_none() && name != "STARTING…" && ui.is_enabled();
    let sense = if interactive { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    let lit = interactive && (response.hovered() || response.has_focus());
    let fill = match (primary, lit) {
        (true, false) => Color32::WHITE,
        (true, true) => Color32::from_gray(228),
        (false, false) => Color32::from_black_alpha(215),
        (false, true) => Color32::from_rgba_unmultiplied(40, 41, 45, 235),
    };
    let radius = 18.0 * scale;
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    let top = rect.center().y - text_height * 0.5;
    painter.galley(pos2(rect.left() + pad.x, top), title.clone(), ink);
    painter.galley(pos2(rect.left() + pad.x, top + title.size().y + 2.0 * scale), about, soft);
    if interactive {
        let at = pos2(rect.right() - pad.x + 4.0 * scale, rect.center().y);
        painter.text(at, Align2::RIGHT_CENTER, "›", egui::FontId::proportional(28.0 * scale), soft);
    }
    if let Some(done) = progress {
        let rail = Rect::from_min_max(
            pos2(rect.left() + pad.x, rect.bottom() - 7.0 * scale),
            pos2(rect.right() - pad.x, rect.bottom() - 4.0 * scale),
        );
        painter.rect_filled(rail, 2.0 * scale, ink.gamma_multiply(0.2));
        let filled = Rect::from_min_max(rail.min, pos2(egui::lerp(rail.x_range(), done.clamp(0.0, 1.0)), rail.max.y));
        painter.rect_filled(filled, 2.0 * scale, ink);
    }
    if response.has_focus() {
        painter.rect_stroke(rect, radius, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
    }
    focus(response)
}

/// The app's own buttons, small and half-transparent next to the modes:
/// an icon, with its name where there is room.
fn icon_button(ui: &mut egui::Ui, icon: &str, label: &str, with_label: bool) -> egui::Response {
    let scale = theme::density(ui);
    let size = (40.0 * scale).max(36.0);
    let text = if with_label { format!("{icon}  {label}") } else { icon.to_owned() };
    let button = egui::Button::new(
        RichText::new(text).size(if with_label { 14.0 } else { 18.0 } * scale).color(Color32::from_white_alpha(220)),
    )
    .fill(Color32::from_black_alpha(140))
    .min_size(vec2(size, size))
    .corner_radius((12.0 * scale) as u8);
    let response = focus(ui.add(button));
    if with_label { response } else { response.on_hover_text(label) }
}

/// What the app knows: the game's files, the last benchmark, why the last
/// session stopped.
fn info(ui: &mut egui::Ui, status: &Status, actions: &mut Vec<Action>) {
    card(ui, |ui| game_files(ui, status));
    if let Some(results) = status.benchmark {
        card(ui, |ui| last_benchmark(ui, results));
    }
    if let Some(why) = status.last_failure {
        card(ui, |ui| last_failure(ui, why, status.graphics_reset, actions));
    }
    ui.label(RichText::new(open_hint()).small().color(theme::MUTED));
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let scale = theme::density(ui);
    egui::Frame::new()
        .fill(Color32::from_black_alpha(210))
        .corner_radius((18.0 * scale) as u8)
        .inner_margin(egui::Margin::symmetric((20.0 * scale) as i8, (16.0 * scale) as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0 * scale;
            add(ui)
        })
        .inner
}

/// A card's title, and on its right a status: a dot and a word.
fn card_title(ui: &mut egui::Ui, title: &str, state: Option<(&str, Color32)>) {
    let scale = theme::density(ui);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0 * scale), Sense::hover());
    let painter = ui.painter();
    let font = egui::FontId::new(17.0 * scale, egui::FontFamily::Name("condensed".into()));
    painter.text(row.left_center(), Align2::LEFT_CENTER, title, font, Color32::WHITE);
    if let Some((word, color)) = state {
        let text = painter.layout_no_wrap(word.into(), egui::FontId::proportional(11.0 * scale), Color32::WHITE);
        let pill = Rect::from_min_max(
            pos2(row.right() - text.size().x - 28.0 * scale, row.center().y - 11.0 * scale),
            pos2(row.right(), row.center().y + 11.0 * scale),
        );
        painter.rect_filled(pill, 11.0 * scale, Color32::from_gray(42));
        painter.circle_filled(pos2(pill.left() + 11.0 * scale, pill.center().y), 3.5 * scale, color);
        painter.galley(pos2(pill.left() + 19.0 * scale, pill.center().y - text.size().y * 0.5), text, Color32::WHITE);
    }
}

fn game_files(ui: &mut egui::Ui, status: &Status) {
    let state = if status.game_dir.is_some() { ("Ready", READY) } else { ("Missing", WARN) };
    card_title(ui, "GAME FILES", Some(state));
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

fn last_benchmark(ui: &mut egui::Ui, results: &crate::benchmark::Results) {
    let scale = theme::density(ui);
    card_title(ui, "LAST BENCHMARK", None);
    ui.label(theme::heading(&format!("{:.0} FPS", results.avg_fps), 34.0 * scale));
    ui.label(
        RichText::new(format!(
            "On average; {:.0} at least, {:.0}% of the frames in under 33 ms.",
            results.min_fps,
            results.smooth * 100.0
        ))
        .small()
        .color(theme::MUTED),
    );
}

fn last_failure(ui: &mut egui::Ui, why: &str, reset: bool, actions: &mut Vec<Action>) {
    card_title(ui, "LAST SESSION", Some(("Stopped", WARN)));
    ui.label(RichText::new(why).small().color(theme::MUTED));
    if reset {
        ui.label(graphics_reset_note());
    } else if ui.horizontal(|ui| action_button(ui, "Reset graphics")).inner.clicked() {
        actions.push(Action::ResetGraphics);
    }
}

/// The settings, over the start page or the game. True when closed.
#[allow(clippy::too_many_arguments)]
pub(super) fn settings_page(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    note: Option<&str>,
    tab: &mut SettingsTab,
    scale_edit: &mut Option<f32>,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) -> bool {
    sheet(ctx, "settings-window", "SETTINGS", |ui| {
        let scale = theme::density(ui);
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
                                egui::Button::new(theme::heading(label, 16.0 * scale).color(if active {
                                    theme::INK
                                } else {
                                    theme::MUTED
                                }))
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
        settings_items(ui, settings, status, *tab, scale_edit, actions, go);
    })
}

/// A window over the start page or the game, centred in the safe area: a
/// title and a close button, then what `add` adds, scrolling. True when
/// closed.
pub(super) fn sheet(ctx: &egui::Context, id: &str, title: &str, add: impl FnOnce(&mut egui::Ui)) -> bool {
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
    egui::Window::new(title)
        .id(Id::new(id))
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
                        ui.label(theme::heading(title, 28.0 * scale));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close = close_button(ui).clicked();
                        });
                    });
                });
            let height = (size.y - ui.min_rect().height()).max(1.0);
            egui::ScrollArea::vertical()
                .content_margin(0)
                .id_salt((id, "scroll"))
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
                        .show(ui, add);
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
    go: &mut Option<Option<Page>>,
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
            if crate::haptics::SUPPORTED {
                setting(ui, "Haptics", Control::Toggle, None, |ui| toggle(ui, &mut settings.haptics));
            }
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
            let next = next_start(started.is_some());
            setting(ui, "Online name", Control::Action, next.or(Some("As the other players see you")), |ui| {
                text_field(ui, &mut settings.online_name, 16)
            });
            setting(ui, "Private lobby", Control::Action, next.or(Some("A code to play with friends only")), |ui| {
                text_field(ui, &mut settings.online_lobby, 32)
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
            let version = concat!("Glue 5 ", env!("CARGO_PKG_VERSION"), " and the projects it builds on");
            setting(ui, "Credits", Control::Action, Some(version), |ui| {
                if action_button(ui, "View").clicked() {
                    *go = Some(Some(Page::Credits));
                }
            });
        }
    }
}

fn next_start(differs: bool) -> Option<&'static str> {
    differs.then_some("Next start")
}

#[cfg(test)]
mod tests {
    use super::{Mode, SettingsTab, Status, settings_page, start_page, theme};
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
            mode: Mode::Story,
            benchmark: None,
        }
    }

    #[test]
    fn start_page_icons_are_in_the_font() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        ctx.begin_pass(Default::default());
        ctx.end_pass().textures_delta.clear();
        assert!(ctx.fonts_mut(|f| f.has_glyphs(&egui::FontId::proportional(14.0), "⚙🗁🗙★›")));
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
                            settings_page(
                                ctx,
                                &mut settings,
                                &status(),
                                None,
                                &mut tab,
                                &mut None,
                                &mut Vec::new(),
                                &mut None,
                            );
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
                    settings_page(
                        root.ctx(),
                        &mut settings,
                        &status(),
                        None,
                        &mut tab,
                        &mut None,
                        &mut Vec::new(),
                        &mut None,
                    );
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
    fn a_controller_lands_on_the_modes_first() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let key = |key| {
            [true, false].map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Default::default(),
            })
        };
        let mut chosen = None;
        for events in
            [vec![], key(egui::Key::Tab).to_vec(), key(egui::Key::ArrowDown).to_vec(), key(egui::Key::Enter).to_vec()]
        {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
                events,
                ..Default::default()
            };
            ctx.run_ui(raw, |root| chosen = start_page(root.ctx(), &status(), None, &mut Vec::new(), &mut None))
                .textures_delta
                .clear();
        }
        assert_eq!(chosen, Some(Mode::Online));
    }

    #[test]
    fn credits_fit_the_safe_area() {
        for size in [vec2(1440.0, 900.0), vec2(390.0, 844.0), vec2(844.0, 390.0), vec2(320.0, 480.0)] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            for _ in 0..4 {
                let raw =
                    egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), ..Default::default() };
                ctx.run_ui(raw, |root| {
                    assert!(!crate::ui::credits_page(root.ctx()));
                })
                .textures_delta
                .clear();
            }
            let window = ctx.memory(|m| m.area_rect(Id::new("credits-window"))).unwrap();
            assert!(ctx.content_rect().contains_rect(window), "{size:?}: {window:?}");
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
                        start_page(root.ctx(), &status(), None, &mut Vec::new(), &mut None);
                    });
                output.textures_delta.clear();
            }
            let home = ctx.memory(|m| m.area_rect(Id::new("start"))).unwrap();
            assert!(screen.contains_rect(home), "{size:?}: {home:?}");
        }
    }
}
