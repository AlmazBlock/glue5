//! The application's own UI, drawn with egui over the game (before the game
//! runs, alone): the start page, the settings, and on touch screens the
//! settings button and the touch controls. The game's menus stay the game's.
//!
//! One window, one page at a time: on a phone a page fills the safe area,
//! on larger screens it is a panel in the middle. The same widgets
//! everywhere, all reachable from a controller or a keyboard: the D-pad,
//! left stick or arrows move the focus, A / Space / Enter activates, B /
//! Esc goes back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, FontId, Id, LayerId, Order, Pos2, Rect, Response, RichText, SafeAreaInsets, Sense, Stroke, Vec2,
    ViewportId, pos2, vec2,
};
use sdl3::event::Event;
use sdl3::keyboard::Scancode;
use sdl3::mouse::MouseButton;
use sdl3::video::Window;

use crate::paths::{self, MOBILE};
use crate::settings::{self, RENDER_SCALES, Settings, TouchMode};
use crate::touch::Touch;

/// A page of the application's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Before the game: its files, Play, Settings.
    Start,
    Settings,
}

/// What the pages ask the application to do.
pub enum Action {
    Play,
    Quit,
    /// The system's folder picker, for the game (desktop).
    ChooseGame,
    /// Look for the game's files again (mobile: copied in meanwhile).
    FindGame,
}

/// Navigation from a controller.
#[derive(Clone, Copy, Debug)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Activate,
    Back,
}

/// What the running game started with: settings that differ apply when it
/// starts again.
pub struct Started {
    pub game_dir: PathBuf,
    pub max_fps: u32,
    pub low_memory: bool,
    pub render_scale: f32,
}

/// What the pages show about the application.
pub struct Status<'a> {
    /// The game's folder, if found.
    pub game_dir: Option<&'a Path>,
    /// Why it was not (or why it would not start).
    pub problem: Option<&'a str>,
    /// The game runs (and started with these).
    pub started: Option<&'a Started>,
    /// The window, in pixels.
    pub window: (u32, u32),
    pub metalfx: bool,
}

/// The settings button over the game: from the safe area's top right
/// corner, in points, sized like the touch controls (next to their Start
/// and Back buttons).
const GEAR_AT: Vec2 = vec2(-156.0, 40.0);
const GEAR_RADIUS: f32 = 20.0;

/// The settings panel; screens with less room than this around it (phones)
/// get the page over the whole safe area.
const PANEL: Vec2 = vec2(560.0, 560.0);
const LABEL_WIDTH: f32 = 130.0;

const ACCENT: Color32 = Color32::from_rgb(52, 120, 230);
const FOCUS: Color32 = Color32::from_rgb(120, 175, 255);
const GOOD: Color32 = Color32::from_rgb(90, 200, 120);
const WARN: Color32 = Color32::from_rgb(240, 180, 70);

pub struct Ui {
    ctx: egui::Context,
    overlay: glue5_gpu::Overlay,
    events: Vec<egui::Event>,
    pointer: Pos2,
    start: Instant,
    page: Option<Page>,
    /// Where the settings go back to: the start page before the game.
    home: Option<Page>,
    /// The last submission drew something (an empty one clears it).
    drawn: bool,
    note: Option<String>,
    /// The render scale while its slider is dragged (it applies when let go).
    scale_edit: Option<f32>,
    /// Play was chosen: this frame shows that it starts, then it does.
    starting: bool,
    /// The settings button as last drawn (centre, radius), in points.
    gear: Option<(Pos2, f32)>,
    /// When egui wants to draw again (before the game, frames are drawn
    /// only for input or for this).
    repaint_at: Option<Instant>,
}

impl Ui {
    pub fn new(overlay: glue5_gpu::Overlay) -> Self {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);
        ctx.all_styles_mut(style);
        Ui {
            ctx,
            overlay,
            events: Vec::new(),
            pointer: Pos2::ZERO,
            start: Instant::now(),
            page: Some(Page::Start),
            home: Some(Page::Start),
            drawn: false,
            note: None,
            scale_edit: None,
            starting: false,
            gear: None,
            repaint_at: Some(Instant::now()),
        }
    }

    pub fn page(&self) -> Option<Page> {
        self.page
    }

    pub fn is_open(&self) -> bool {
        self.page.is_some()
    }

    /// The game runs: no start page any more.
    pub fn game_started(&mut self) {
        self.home = None;
        self.starting = false;
        self.show(None);
    }

    /// The settings (from the game or the start page); again, back.
    pub fn toggle_settings(&mut self) {
        if self.page == Some(Page::Settings) {
            self.back();
        } else {
            self.show(Some(Page::Settings));
        }
    }

    /// From the settings back to the start page, or the game.
    pub fn back(&mut self) {
        if self.page == Some(Page::Settings) {
            self.show(self.home);
        }
    }

    fn show(&mut self, page: Option<Page>) {
        self.page = page;
        self.note = None;
        self.scale_edit = None;
        self.ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        self.repaint();
    }

    /// A message shown in the settings (e.g. "applies at the next start").
    pub fn note(&mut self, text: impl Into<String>) {
        self.note = Some(text.into());
        self.repaint();
    }

    /// Draw again at the next frame (the window changed, ...).
    pub fn repaint(&mut self) {
        self.repaint_in(Duration::ZERO);
    }

    pub fn repaint_in(&mut self, delay: Duration) {
        let at = Instant::now() + delay;
        self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
    }

    /// Before the game: whether a frame should be drawn now, and how long
    /// the application can wait for input otherwise.
    pub fn wants_frame(&self) -> bool {
        !self.events.is_empty() || self.repaint_at.is_some_and(|t| Instant::now() >= t)
    }

    pub fn idle_for(&self) -> Duration {
        let most = Duration::from_millis(250);
        self.repaint_at.map_or(most, |t| t.saturating_duration_since(Instant::now()).min(most))
    }

    /// Whether a finger at `x`, `y` (0..1, as SDL gives them) is on the
    /// settings button.
    pub fn gear_hit(&self, x: f32, y: f32, window: &Window) -> bool {
        let (w, h) = window.size();
        self.gear.is_some_and(|(c, r)| c.distance(pos2(x * w as f32, y * h as f32)) <= r * 1.4)
    }

    /// Input for a page while one is open.
    pub fn event(&mut self, event: &Event, window: &Window) {
        let (w, h) = window.size();
        let button = |b: &MouseButton| match b {
            MouseButton::Right => Some(egui::PointerButton::Secondary),
            MouseButton::Middle => Some(egui::PointerButton::Middle),
            MouseButton::Left => Some(egui::PointerButton::Primary),
            _ => None,
        };
        let modifiers = egui::Modifiers::default();
        match event {
            Event::MouseMotion { x, y, .. } => self.pointer_to(pos2(*x, *y)),
            Event::MouseButtonDown { mouse_btn, x, y, .. } | Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                let pressed = matches!(event, Event::MouseButtonDown { .. });
                if let Some(button) = button(mouse_btn) {
                    self.pointer_to(pos2(*x, *y));
                    self.events.push(egui::Event::PointerButton { pos: self.pointer, button, pressed, modifiers });
                }
            }
            Event::MouseWheel { x, y, .. } => self.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(*x, *y),
                modifiers,
                phase: egui::TouchPhase::Move,
            }),
            // A finger is the pointer.
            Event::FingerDown { x, y, .. } | Event::FingerUp { x, y, .. } => {
                let pressed = matches!(event, Event::FingerDown { .. });
                self.pointer_to(pos2(x * w as f32, y * h as f32));
                self.events.push(egui::Event::PointerButton {
                    pos: self.pointer,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                });
                if !pressed {
                    self.events.push(egui::Event::PointerGone);
                }
            }
            Event::FingerMotion { x, y, .. } => self.pointer_to(pos2(x * w as f32, y * h as f32)),
            Event::TextInput { text, .. } => self.events.push(egui::Event::Text(text.clone())),
            Event::KeyDown { scancode: Some(sc), keymod, .. } | Event::KeyUp { scancode: Some(sc), keymod, .. } => {
                let pressed = matches!(event, Event::KeyDown { .. });
                if let Some(key) = key(*sc) {
                    let shift = keymod.intersects(sdl3::keyboard::Mod::LSHIFTMOD | sdl3::keyboard::Mod::RSHIFTMOD);
                    self.key(key, pressed, egui::Modifiers { shift, ..Default::default() });
                }
            }
            _ => {}
        }
    }

    fn pointer_to(&mut self, p: Pos2) {
        self.pointer = p;
        self.events.push(egui::Event::PointerMoved(p));
    }

    fn key(&mut self, key: egui::Key, pressed: bool, modifiers: egui::Modifiers) {
        self.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    }

    pub fn nav(&mut self, nav: Nav) {
        let key = match nav {
            Nav::Up => egui::Key::ArrowUp,
            Nav::Down => egui::Key::ArrowDown,
            Nav::Left => egui::Key::ArrowLeft,
            Nav::Right => egui::Key::ArrowRight,
            Nav::Activate => egui::Key::Enter,
            Nav::Back => return self.back(),
        };
        // Nothing has the focus yet: the first move gives it to the page's
        // first control (egui moves it from a focused widget only).
        let key = if self.ctx.memory(|m| m.focused()).is_none() { egui::Key::Tab } else { key };
        self.key(key, true, Default::default());
        self.key(key, false, Default::default());
    }

    /// Draws the open page, or over the game the touch controls (if given)
    /// and the settings button (if given, at this opacity).
    pub fn frame(
        &mut self,
        window: &Window,
        settings: &mut Settings,
        status: &Status,
        touch: Option<&Touch>,
        gear: Option<f32>,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let gear = gear.filter(|_| self.page.is_none());
        if self.page.is_none() && touch.is_none() && gear.is_none() {
            if self.drawn {
                self.overlay.submit(Vec::new(), Default::default(), 1.0);
                self.drawn = false;
            }
            self.events.clear();
            self.gear = None;
            return actions;
        }
        // Play was chosen at the last frame: this one shows that the game
        // starts, then it does.
        let starting = std::mem::take(&mut self.starting);
        let (w, h) = window.size();
        let ppp = window.display_scale().max(0.5);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32));
        let safe = safe_rect(window);
        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            safe_area_insets: Some(SafeAreaInsets(egui::epaint::MarginF32 {
                left: safe.min.x - screen.min.x,
                right: screen.max.x - safe.max.x,
                top: safe.min.y - screen.min.y,
                bottom: screen.max.y - safe.max.y,
            })),
            time: Some(self.start.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        };
        raw.viewports.entry(ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);

        let ctx = self.ctx.clone();
        let mut go = None;
        ctx.begin_pass(raw);
        if let Some(touch) = touch {
            touch.draw(&ctx.layer_painter(LayerId::background()), settings.touch_opacity);
        }
        self.gear = gear.map(|alpha| draw_gear(&ctx, safe, settings.touch_size, alpha));
        match self.page {
            Some(Page::Start) => {
                if start_page(&ctx, status, starting, &mut actions, &mut go) {
                    self.starting = true;
                }
            }
            Some(Page::Settings) => {
                if status.started.is_some() {
                    // The game behind, dimmed.
                    ctx.layer_painter(LayerId::new(Order::Background, Id::new("dim"))).rect_filled(
                        screen,
                        0.0,
                        Color32::from_black_alpha(150),
                    );
                }
                let note = self.note.as_deref();
                if settings_page(&ctx, settings, status, note, &mut self.scale_edit, &mut actions) {
                    go = Some(self.home);
                }
            }
            None => {}
        }
        let output = ctx.end_pass();
        let delay = output.viewport_output.get(&ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        self.repaint_at = (delay < Duration::from_secs(10)).then(|| Instant::now() + delay);
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        self.overlay.submit(primitives, output.textures_delta, output.pixels_per_point);
        self.drawn = true;
        if let Some(page) = go {
            self.show(page);
        }
        if self.starting {
            self.repaint();
        }
        if starting {
            actions.push(Action::Play);
        }
        actions
    }
}

fn style(s: &mut egui::Style) {
    use egui::TextStyle;
    s.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
    s.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
    s.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0));
    s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
    s.spacing.item_spacing = vec2(10.0, 8.0);
    s.spacing.button_padding = vec2(12.0, 6.0);
    s.spacing.interact_size.y = if MOBILE { 38.0 } else { 28.0 };
    s.spacing.slider_width = 180.0;
    s.visuals.window_corner_radius = 14.into();
    s.visuals.selection.bg_fill = ACCENT;
    // The focus (controller, keyboard) stands out.
    s.visuals.widgets.active.bg_stroke = Stroke::new(2.0, FOCUS);
}

/// The part of the window clear of notches and rounded corners, in points.
pub fn safe_rect(window: &Window) -> Rect {
    let (w, h) = window.size();
    match window.safe_area() {
        Ok(r) if r.width() > 0 && r.height() > 0 => {
            Rect::from_min_size(pos2(r.x() as f32, r.y() as f32), vec2(r.width() as f32, r.height() as f32))
        }
        _ => Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32)),
    }
}

/// The settings button: the touch controls' look, at their size.
fn draw_gear(ctx: &egui::Context, safe: Rect, scale: f32, alpha: f32) -> (Pos2, f32) {
    let c = pos2(safe.max.x + GEAR_AT.x * scale, safe.min.y + GEAR_AT.y * scale);
    let r = GEAR_RADIUS * scale;
    let white = |a: f32| Color32::from_white_alpha((a * alpha * 255.0) as u8);
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("gear")));
    painter.circle(c, r, white(0.18), Stroke::new(1.5 * scale, white(0.6)));
    painter.text(c, Align2::CENTER_CENTER, "⚙", FontId::proportional(r * 0.95), white(0.9));
    (c, r)
}

/// How to open the settings in the game.
fn open_hint() -> &'static str {
    if MOBILE {
        "In the game: ⚙ at the top right, or hold Back + Start on a controller."
    } else {
        "In the game: F1, or hold Back + Start on a controller."
    }
}

/// The start page; true when Play is chosen.
fn start_page(
    ctx: &egui::Context,
    status: &Status,
    starting: bool,
    actions: &mut Vec<Action>,
    go: &mut Option<Option<Page>>,
) -> bool {
    let mut play = false;
    let width = 400f32.min(ctx.content_rect().width() - 32.0);
    egui::Area::new(Id::new("start")).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.set_width(width);
        ui.vertical_centered(|ui| ui.label(RichText::new("Glue 5").size(34.0).strong()));
        ui.add_space(12.0);
        card(ui, |ui| game_files(ui, status));
        ui.add_space(12.0);
        let label = if starting { "Starting…" } else { "Play" };
        let button = egui::Button::new(RichText::new(label).size(19.0).strong().color(Color32::WHITE)).fill(ACCENT);
        ui.add_enabled_ui(status.game_dir.is_some() && !starting, |ui| {
            play = focus(ui.add_sized([width, 50.0], button)).clicked();
        });
        let half = (width - ui.spacing().item_spacing.x) / 2.0;
        ui.horizontal(|ui| {
            if focus(ui.add_sized([half, 40.0], egui::Button::new("Settings"))).clicked() {
                *go = Some(Some(Page::Settings));
            }
            let (label, action) =
                if MOBILE { ("Look again", Action::FindGame) } else { ("Game folder…", Action::ChooseGame) };
            if focus(ui.add_sized([half, 40.0], egui::Button::new(label))).clicked() {
                actions.push(action);
            }
        });
        if !MOBILE && focus(ui.add_sized([width, 34.0], egui::Button::new("Quit"))).clicked() {
            actions.push(Action::Quit);
        }
        ui.add_space(8.0);
        ui.vertical_centered(|ui| ui.label(RichText::new(open_hint()).small().weak()));
    });
    play
}

fn game_files(ui: &mut egui::Ui, status: &Status) {
    match status.game_dir {
        Some(dir) => {
            dot_title(ui, GOOD, "Game files found");
            ui.label(RichText::new(dir.display().to_string()).small().weak());
        }
        None => {
            dot_title(ui, WARN, "Game files not found");
            if let Some(problem) = status.problem {
                ui.label(RichText::new(problem).color(WARN));
            }
            ui.label(paths::where_the_game_goes());
            ui.label(RichText::new(paths::game_layout()).monospace().small().weak());
        }
    }
}

fn dot_title(ui: &mut egui::Ui, color: Color32, title: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
        ui.label(RichText::new(title).strong());
    });
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(10)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The settings page; true when it is closed.
fn settings_page(
    ctx: &egui::Context,
    settings: &mut Settings,
    status: &Status,
    note: Option<&str>,
    scale_edit: &mut Option<f32>,
    actions: &mut Vec<Action>,
) -> bool {
    let area = ctx.content_rect();
    let phone = area.width() < PANEL.x + 48.0 || area.height() < PANEL.y;
    let margin = if phone { 14.0 } else { 20.0 };
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(margin);
    let window = egui::Window::new("Settings").title_bar(false).resizable(false).collapsible(false).frame(frame);
    let (window, height) = if phone {
        (window.fixed_rect(area), area.height())
    } else {
        let height = (area.height() - 64.0).min(PANEL.y);
        (window.anchor(Align2::CENTER_CENTER, [0.0, 0.0]).fixed_size([PANEL.x, height]), height)
    };
    let mut close = false;
    window.show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Settings");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let done = if status.started.is_some() { "Back to the game" } else { "Done" };
                if focus(ui.button(done)).clicked() {
                    close = true;
                }
            });
        });
        if let Some(note) = note {
            ui.label(RichText::new(note).color(WARN));
        }
        ui.separator();
        let left = height - ui.min_rect().height() - 2.0 * margin;
        egui::ScrollArea::vertical()
            .max_height(left.max(80.0))
            .auto_shrink([false, false])
            .show(ui, |ui| settings_items(ui, settings, status, scale_edit, actions));
    });
    close
}

fn settings_items(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    status: &Status,
    scale_edit: &mut Option<f32>,
    actions: &mut Vec<Action>,
) {
    let started = status.started;
    section(ui, "Video");
    grid(ui, "video", |ui| {
        ui.label("Resolution");
        let mut scale = scale_edit.unwrap_or(settings.render_scale());
        let slider = egui::Slider::new(&mut scale, RENDER_SCALES)
            .step_by(0.05)
            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0));
        let r = focus(ui.add(slider));
        // Applied when let go: the game rebuilds its render targets for each
        // new size.
        if r.dragged() {
            *scale_edit = Some(scale);
        } else if r.changed() || r.drag_stopped() {
            settings.render_scale = Some(settings::round_scale(scale));
            *scale_edit = None;
        }
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            let (rw, rh) = settings::render_size(status.window, scale, settings.low_memory);
            let (ww, wh) = status.window;
            ui.label(RichText::new(format!("{rw} × {rh} for a window of {ww} × {wh}")).small().weak());
            // Phone windows never change size: the engine's device reset
            // never runs there, and a new size waits for the next start.
            later(ui, MOBILE && started.is_some_and(|s| s.render_scale != settings.render_scale()));
        });
        ui.end_row();
        if status.metalfx {
            ui.label("Upscaling");
            choice(ui, &mut settings.metalfx, &[(false, "Bicubic"), (true, "MetalFX")])
                .on_hover_text("How the game's image is scaled up to the window");
            ui.end_row();
        }
        ui.label("VSync");
        toggle(ui, &mut settings.vsync);
        ui.end_row();
        ui.label("Frame rate");
        ui.horizontal(|ui| {
            let unlimited = if settings.vsync { "Display" } else { "Unlimited" };
            choice(ui, &mut settings.max_fps, &[(0, unlimited), (60, "60"), (30, "30")]);
            later(ui, started.is_some_and(|s| s.max_fps != settings.max_fps));
        });
        ui.end_row();
    });

    section(ui, "Controls");
    grid(ui, "controls", |ui| {
        ui.label("Touch controls");
        choice(
            ui,
            &mut settings.touch,
            &[(TouchMode::Auto, "Auto"), (TouchMode::Always, "Always"), (TouchMode::Off, "Off")],
        )
        .on_hover_text("Auto: on touch screens, hidden while a controller or keyboard is used");
        ui.end_row();
        ui.label("Size");
        focus(ui.add(egui::Slider::new(&mut settings.touch_size, 0.7..=1.5).fixed_decimals(1)));
        ui.end_row();
        ui.label("Opacity");
        focus(ui.add(egui::Slider::new(&mut settings.touch_opacity, 0.2..=1.0).fixed_decimals(1)));
        ui.end_row();
    });
    ui.label(RichText::new(open_hint()).small().weak());

    section(ui, "Game");
    grid(ui, "game", |ui| {
        ui.label("Low memory");
        ui.horizontal(|ui| {
            if toggle(ui, &mut settings.low_memory).changed() && settings.low_memory {
                settings.low_preset_written = false;
            }
            later(ui, started.is_some_and(|s| s.low_memory != settings.low_memory));
        });
        ui.end_row();
        ui.label("");
        ui.label(
            RichText::new("Smaller engine pools. Turning it on lowers the game's graphics settings once.")
                .small()
                .weak(),
        );
        ui.end_row();
        ui.label("Folder");
        ui.vertical(|ui| {
            // Before the game, the folder found; in it, the one the next start uses.
            let dir = if started.is_some() { settings.game_dir.as_deref() } else { status.game_dir };
            let shown = dir.map_or_else(|| "not found".to_owned(), |d| d.display().to_string());
            ui.label(RichText::new(shown).small().weak());
            ui.horizontal(|ui| {
                if !MOBILE && focus(ui.button("Choose…")).clicked() {
                    actions.push(Action::ChooseGame);
                }
                later(ui, started.is_some_and(|s| Some(s.game_dir.as_path()) != settings.game_dir.as_deref()));
            });
        });
        ui.end_row();
    });
    ui.label(
        RichText::new("Graphics quality is the game's own: its menu, Settings › Graphics (Esc / Start).")
            .small()
            .weak(),
    );
    if started.is_some() && !MOBILE {
        ui.add_space(12.0);
        if focus(ui.button("Quit the game")).clicked() {
            actions.push(Action::Quit);
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title.to_uppercase()).small().strong().color(ui.visuals().weak_text_color()));
    ui.add_space(2.0);
}

fn grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).min_col_width(LABEL_WIDTH).spacing(vec2(16.0, 10.0)).show(ui, add);
}

/// A focusable widget: brought into view when it gets the focus (moved there
/// by a controller or the keyboard).
fn focus(r: Response) -> Response {
    if r.gained_focus() {
        r.scroll_to_me(None);
    }
    r
}

/// "next start" beside a setting changed while the game runs that applies
/// when it starts again.
fn later(ui: &mut egui::Ui, differs: bool) {
    if differs {
        ui.label(RichText::new("next start").small().color(WARN));
    }
}

/// One of a few values, side by side on a common ground.
fn choice<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> Response {
    let ground = egui::Frame::new().fill(ui.visuals().widgets.inactive.weak_bg_fill).corner_radius(6).inner_margin(2);
    ground
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let mut all: Option<Response> = None;
                for &(v, label) in options {
                    let r = focus(ui.selectable_label(*value == v, label));
                    if r.clicked() {
                        *value = v;
                    }
                    all = Some(match all {
                        Some(a) => a | r,
                        None => r,
                    });
                }
                all.expect("options")
            })
            .inner
        })
        .inner
}

/// An on/off switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> Response {
    let h = 24.0;
    let (rect, mut r) = ui.allocate_exact_size(vec2(h * 1.8, h), Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let value = *on;
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, value, ""));
    let t = ui.ctx().animate_bool_responsive(r.id, value);
    let visuals = ui.style().interact_selectable(&r, value);
    let fill = if value { ACCENT } else { ui.visuals().widgets.inactive.bg_fill };
    let radius = h / 2.0;
    ui.painter().rect(rect, radius, fill, visuals.bg_stroke, egui::StrokeKind::Outside);
    let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
    ui.painter().circle_filled(pos2(x, rect.center().y), radius * 0.75, Color32::WHITE);
    focus(r)
}

fn key(sc: Scancode) -> Option<egui::Key> {
    Some(match sc {
        Scancode::Tab => egui::Key::Tab,
        Scancode::Return | Scancode::KpEnter => egui::Key::Enter,
        Scancode::Space => egui::Key::Space,
        Scancode::Backspace => egui::Key::Backspace,
        Scancode::Up => egui::Key::ArrowUp,
        Scancode::Down => egui::Key::ArrowDown,
        Scancode::Left => egui::Key::ArrowLeft,
        Scancode::Right => egui::Key::ArrowRight,
        _ => return None,
    })
}
