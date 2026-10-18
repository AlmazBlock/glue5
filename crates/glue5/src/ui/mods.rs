//! Mods: the mods in the user directory's `mods` folder, started and
//! stopped here; GLUE 5 Studio (the web IDE the game serves) with its
//! address, QR code and pairing code; the mods' log.

use egui::{Color32, CornerRadius, RichText, Sense, vec2};
use glue5_mods::ModState;

use super::pages::{card_title, sheet, small};
use super::widgets::{Control, action_button, choice, segmented, setting, slider, toggle};
use super::{Status, WARN, theme};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    Menus,
    #[default]
    Mods,
    Studio,
    Log,
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct ModsPage {
    tab: Tab,
}

impl ModsPage {
    /// Opens on the mods' menus.
    pub fn show_menus(&mut self) {
        self.tab = Tab::Menus;
    }
}

/// The Studio as it serves, for the page.
pub struct StudioInfo {
    /// The page's address, the pairing code in it.
    pub url: String,
    /// The pairing code as people type it.
    pub code: String,
    /// The MCP endpoint, for coding agents.
    pub mcp: String,
    pub lan: bool,
}

const OK: Color32 = Color32::from_rgb(120, 205, 145);
const ERR: Color32 = Color32::from_rgb(240, 149, 149);

/// True when closed.
pub(super) fn mods_page(ctx: &egui::Context, page: &mut ModsPage, settings: &mut Settings, status: &Status) -> bool {
    // Mods change by themselves (they end, fail, the Studio starts them).
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    sheet(ctx, "mods-window", "MODS", |ui| {
        let scale = theme::density(ui);
        segmented(
            ui,
            &mut page.tab,
            &[(Tab::Menus, "MENUS"), (Tab::Mods, "MODS"), (Tab::Studio, "STUDIO"), (Tab::Log, "LOG")],
            40.0 * scale,
        );
        ui.add_space(10.0 * scale);
        if !status.mods {
            let text = if settings.mods {
                "Mods run from the next start of the game."
            } else {
                "Mods are off. Turn them on in Settings › Game › Mods; they run from the next start."
            };
            muted(ui, text);
            return;
        }
        match page.tab {
            Tab::Menus => menus_tab(ui),
            Tab::Mods => mods_tab(ui),
            Tab::Studio => studio_tab(ui, settings, status),
            Tab::Log => log_tab(ui),
        }
    })
}

/// The mods' menus, each under its title: their items with the app's own
/// widgets; what the player does goes to the mod.
fn menus_tab(ui: &mut egui::Ui) {
    let scale = theme::density(ui);
    let menus = glue5_mods::menus();
    if menus.is_empty() {
        muted(ui, "No menus. A mod makes one with ui.menu(\"Title\"); F7 opens them during the game.");
        return;
    }
    for menu in &menus {
        ui.add_space(4.0 * scale);
        ui.horizontal(|ui| {
            ui.label(theme::heading(&menu.title.to_uppercase(), 17.0 * scale).color(Color32::WHITE));
            ui.label(RichText::new(&menu.module).size(12.0 * scale).color(theme::MUTED));
        });
        ui.push_id(menu.id, |ui| {
            for item in &menu.items {
                ui.push_id(item.id, |ui| menu_item(ui, &menu.module, item));
            }
        });
    }
}

fn menu_item(ui: &mut egui::Ui, module: &str, item: &glue5_mods::MenuItem) {
    use glue5_mods::{MenuInput, MenuKind};
    let note = item.note.as_deref();
    match &item.kind {
        MenuKind::Toggle(on) => {
            let mut v = *on;
            setting(ui, &item.label, Control::Toggle, note, |ui| {
                if toggle(ui, &mut v).changed() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Toggle(v));
                }
            });
        }
        MenuKind::Slider { value, min, max, step } => {
            let mut v = *value as f32;
            let decimals = if step.fract() == 0.0 { 0 } else { 2 };
            setting(ui, &item.label, Control::Slider, note, |ui| {
                if slider(ui, &mut v, *min as f32..=*max as f32, *step, "", decimals).changed() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Value(f64::from(v)));
                }
            });
        }
        MenuKind::Choice { options, index } => {
            let mut i = *index;
            let labelled: Vec<(usize, &str)> = options.iter().enumerate().map(|(n, o)| (n, o.as_str())).collect();
            setting(ui, &item.label, Control::Choice, note, |ui| {
                if choice(ui, &mut i, &labelled).changed() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Choose(i));
                }
            });
        }
        MenuKind::Button => {
            setting(ui, &item.label, Control::Action, note, |ui| {
                if action_button(ui, "Run").clicked() {
                    glue5_mods::menu_input(module, item.id, MenuInput::Press);
                }
            });
        }
        MenuKind::Label => muted(ui, &item.label),
    }
}

fn mods_tab(ui: &mut egui::Ui) {
    let held = glue5_mods::held_back();
    if !held.is_empty() {
        small(
            ui,
            &format!("{} did not start with the game: it was running when the game stopped last time.", held.join(", ")),
            WARN,
        );
    }
    let mods = glue5_mods::mods();
    if mods.is_empty() {
        muted(
            ui,
            "No mods yet. Make one in GLUE 5 Studio (the STUDIO tab), or put a folder with a main.luau in the mods folder.",
        );
    }
    for m in &mods {
        let subtitle = match m.state {
            ModState::Running if m.waiting => "Waiting".to_owned(),
            ModState::Running => format!("Running · {:.1} ms per frame", m.cpu_ms),
            ModState::Error => format!("Error · {}", m.error.as_deref().unwrap_or("")),
            ModState::Stopped if m.autostart => "Stopped · starts with the game".to_owned(),
            ModState::Stopped => "Stopped".to_owned(),
        };
        let dot = match m.state {
            ModState::Running => "• ",
            _ => "",
        };
        setting(ui, &format!("{dot}{}", m.name), Control::Action, Some(&subtitle), |ui| {
            let running = m.state == ModState::Running;
            if action_button(ui, if running { "Stop" } else { "Start" }).clicked() {
                if running {
                    glue5_mods::stop(&m.name);
                } else {
                    glue5_mods::start(&m.name);
                }
            }
        });
    }
    let running: Vec<_> = mods.iter().filter(|m| m.state == ModState::Running).collect();
    if !running.is_empty() {
        let total: f32 = running.iter().map(|m| m.cpu_ms).sum();
        setting(ui, &format!("{} running", running.len()), Control::Action, Some(&format!("{total:.1} ms per frame")), |ui| {
            if action_button(ui, "Stop all").clicked() {
                for m in &running {
                    glue5_mods::stop(&m.name);
                }
            }
        });
    }
    if glue5_mods::tainted() {
        small(ui, "A native failed inside the game: restart the game before long.", WARN);
    }
}

fn studio_tab(ui: &mut egui::Ui, settings: &mut Settings, status: &Status) {
    let scale = theme::density(ui);
    setting(ui, "GLUE 5 Studio", Control::Toggle, Some("Edit, run and debug mods from a browser"), |ui| {
        toggle(ui, &mut settings.studio);
    });
    if let Some(info) = &status.studio {
        panel(ui, |ui| {
            card_title(ui, "OPEN ON A COMPUTER OR TABLET", Some(("On", OK)));
            ui.horizontal_top(|ui| {
                qr(ui, &info.url, 148.0 * scale);
                ui.add_space(12.0 * scale);
                ui.vertical(|ui| {
                    small(
                        ui,
                        if info.lan {
                            "Scan the code, or open the address in a browser on the same network."
                        } else {
                            "Open the address in a browser on this device."
                        },
                        theme::MUTED,
                    );
                    ui.add_space(6.0 * scale);
                    ui.label(RichText::new(&info.url).size(13.0 * scale).color(Color32::WHITE));
                    ui.add_space(6.0 * scale);
                    ui.label(RichText::new(format!("Pairing code  {}", info.code)).size(14.0 * scale).color(Color32::WHITE));
                    ui.add_space(8.0 * scale);
                    // A button's width, not the row's (it lays out right to left).
                    let copy = ui.allocate_ui(vec2(150.0 * scale, 44.0 * scale), |ui| action_button(ui, "Copy address")).inner;
                    if copy.clicked() {
                        ui.ctx().copy_text(info.url.clone());
                    }
                });
            });
        });
        let command = format!(
            "claude mcp add --transport http glue5 {} --header \"Authorization: Bearer {}\"",
            info.mcp,
            info.code.replace('-', "")
        );
        setting(ui, "Claude Code", Control::Action, Some("Lets it write, run and test mods (MCP)"), |ui| {
            if action_button(ui, "Copy command").clicked() {
                ui.ctx().copy_text(command.clone());
            }
        });
    } else if settings.studio {
        small(ui, "Starting…", theme::MUTED);
    }
    setting(
        ui,
        "Other devices",
        Control::Toggle,
        Some(if settings.studio_lan { "Browsers on this network can connect, with the code" } else { "Only a browser on this device" }),
        |ui| {
            toggle(ui, &mut settings.studio_lan);
        },
    );
    muted(
        ui,
        "Whoever has the code can run code in the game: it changes each time the Studio starts. Mods can do \
         anything a game script can.",
    );
}

fn log_tab(ui: &mut egui::Ui) {
    let scale = theme::density(ui);
    let (lines, _) = glue5_mods::logs(0);
    if lines.is_empty() {
        muted(ui, "Nothing yet.");
    }
    panel(ui, |ui| {
        for l in lines.iter().rev().take(80).rev() {
            let color = if l.level == glue5_mods::LogLevel::Error { ERR } else { Color32::from_gray(213) };
            ui.label(RichText::new(format!("{} · {}", l.module, l.text)).monospace().size(12.0 * scale).color(color));
        }
    });
}

/// `text` as a QR code, dark on white.
fn qr(ui: &mut egui::Ui, text: &str, size: f32) {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else { return };
    let modules = code.width();
    let colors = code.to_colors();
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same((10.0 * theme::density(ui)) as u8), Color32::WHITE);
    let quiet = 2.0;
    let cell = size / (modules as f32 + 2.0 * quiet);
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i % modules) as f32, (i / modules) as f32);
            let min = rect.min + vec2((x + quiet) * cell, (y + quiet) * cell);
            painter.rect_filled(egui::Rect::from_min_size(min, vec2(cell + 0.5, cell + 0.5)), 0, theme::INK);
        }
    }
}

fn panel(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let scale = theme::density(ui);
    egui::Frame::new()
        .fill(Color32::from_gray(28))
        .corner_radius((14.0 * scale) as u8)
        .inner_margin((14.0 * scale) as i8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0 * scale;
            add(ui);
        });
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0 * theme::density(ui)).color(theme::MUTED));
}
