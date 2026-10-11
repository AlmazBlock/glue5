//! Touch controls: an on-screen gamepad. Fingers on the left half move a
//! stick that appears where the thumb lands; the right half drags the camera
//! (right stick) and holds the buttons. What it produces is a [`Gamepad`], so
//! the game plays exactly as with a controller (its prompts, aim assist).

use std::collections::HashMap;
use std::time::Instant;

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use glue5_runtime::input::{Gamepad, button};

/// Stick travel, in points (scaled by the size setting).
const STICK_RADIUS: f32 = 56.0;
/// Camera speed: points of finger travel per second for a full right stick.
const LOOK_SPEED: f32 = 700.0;

#[derive(Clone, Copy)]
struct Button {
    icon: Icon,
    bits: u16,
    /// Centre, from the right (x < 0) or left (x > 0) edge and the bottom
    /// (y < 0) or top (y > 0) edge of the safe area, in points.
    at: (f32, f32),
    radius: f32,
}

#[derive(Clone, Copy)]
enum Icon {
    Text(&'static str),
    /// A triangle pointing this way.
    Arrow(Vec2),
    /// Two overlapping windows (Back / View).
    Windows,
}

const fn b(label: &'static str, bits: u16, at: (f32, f32), radius: f32) -> Button {
    Button { icon: Icon::Text(label), bits, at, radius }
}

const fn arrow(x: f32, y: f32, bits: u16, at: (f32, f32)) -> Button {
    Button { icon: Icon::Arrow(vec2(x, y)), bits, at, radius: 22.0 }
}

/// Layout for the right thumb (face buttons, triggers) and the edges
/// (system buttons, phone, crouch).
const BUTTONS: &[Button] = &[
    b("A", button::SOUTH, (-92.0, -62.0), 28.0),
    b("B", button::EAST, (-40.0, -112.0), 28.0),
    b("X", button::WEST, (-144.0, -112.0), 28.0),
    b("Y", button::NORTH, (-92.0, -162.0), 28.0),
    b("RT", button::R2, (-58.0, -236.0), 30.0),
    b("RB", button::R1, (-136.0, -236.0), 24.0),
    b("LT", button::L2, (-226.0, -172.0), 30.0),
    b("LB", button::L1, (-226.0, -246.0), 24.0),
    b("R3", button::R3, (-226.0, -96.0), 20.0),
    b("L3", button::L3, (40.0, -40.0), 20.0),
    arrow(0.0, -1.0, button::UP, (44.0, 120.0)),
    arrow(-1.0, 0.0, button::LEFT, (96.0, 120.0)),
    arrow(1.0, 0.0, button::RIGHT, (148.0, 120.0)),
    arrow(0.0, 1.0, button::DOWN, (200.0, 120.0)),
    b("☰", button::START, (-44.0, 40.0), 22.0),
    Button { icon: Icon::Windows, bits: button::SELECT, at: (-100.0, 40.0), radius: 22.0 },
];

#[derive(Clone, Copy, Debug)]
enum Finger {
    Stick { origin: Pos2, at: Pos2 },
    Look { last: Pos2, last_t: Instant, velocity: Vec2 },
    Button(usize),
}

pub struct Touch {
    fingers: HashMap<u64, Finger>,
    /// Points per unit of SDL's normalised finger coordinates.
    screen: Vec2,
    /// Where the controls go: clear of notches and rounded corners.
    area: Rect,
    scale: f32,
}

impl Touch {
    pub fn new() -> Self {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0));
        Touch { fingers: HashMap::new(), screen: area.size(), area, scale: 1.0 }
    }

    /// The window's size and safe area in points, and the controls' size
    /// setting.
    pub fn layout(&mut self, screen: Vec2, area: Rect, scale: f32) {
        self.screen = screen;
        self.area = area;
        self.scale = scale;
    }

    fn center(&self, b: &Button) -> Pos2 {
        let (s, a) = (self.scale, self.area);
        let x = if b.at.0 < 0.0 { a.max.x + b.at.0 * s } else { a.min.x + b.at.0 * s };
        let y = if b.at.1 < 0.0 { a.max.y + b.at.1 * s } else { a.min.y + b.at.1 * s };
        pos2(x, y)
    }

    fn hit(&self, b: &Button, p: Pos2) -> bool {
        // A little more generous than drawn.
        self.center(b).distance(p) <= b.radius * self.scale * 1.25
    }

    pub fn finger_down(&mut self, id: u64, x: f32, y: f32) {
        let p = pos2(x * self.screen.x, y * self.screen.y);
        let finger = if let Some(i) = BUTTONS.iter().position(|b| self.hit(b, p)) {
            Finger::Button(i)
        } else if p.x < self.screen.x * 0.45 {
            Finger::Stick { origin: p, at: p }
        } else {
            Finger::Look { last: p, last_t: Instant::now(), velocity: Vec2::ZERO }
        };
        self.fingers.insert(id, finger);
    }

    pub fn finger_motion(&mut self, id: u64, x: f32, y: f32) {
        let p = pos2(x * self.screen.x, y * self.screen.y);
        match self.fingers.get_mut(&id) {
            Some(Finger::Stick { at, .. }) => *at = p,
            Some(Finger::Look { last, last_t, velocity }) => {
                let dt = last_t.elapsed().as_secs_f32().max(1e-3);
                // Smoothed: motion events arrive unevenly.
                *velocity = *velocity * 0.5 + (p - *last) / dt * 0.5;
                *last = p;
                *last_t = Instant::now();
            }
            _ => {}
        }
    }

    pub fn finger_up(&mut self, id: u64) {
        self.fingers.remove(&id);
    }

    pub fn clear(&mut self) {
        self.fingers.clear();
    }

    /// The gamepad the fingers make now.
    pub fn gamepad(&mut self) -> Gamepad {
        let mut pad = Gamepad { connected: true, ..Default::default() };
        let radius = STICK_RADIUS * self.scale;
        for finger in self.fingers.values_mut() {
            match finger {
                Finger::Stick { origin, at } => {
                    let v = (*at - *origin) / radius;
                    let v = if v.length() > 1.0 { v / v.length() } else { v };
                    pad.left = [v.x, v.y];
                }
                Finger::Look { last_t, velocity, .. } => {
                    // A finger resting stops the camera.
                    if last_t.elapsed().as_secs_f32() > 0.05 {
                        *velocity = Vec2::ZERO;
                    }
                    let v = *velocity / LOOK_SPEED;
                    pad.right = [v.x.clamp(-1.0, 1.0), v.y.clamp(-1.0, 1.0)];
                }
                Finger::Button(i) => {
                    pad.buttons |= BUTTONS[*i].bits;
                    if BUTTONS[*i].bits == button::L2 {
                        pad.l2 = 1.0;
                    }
                    if BUTTONS[*i].bits == button::R2 {
                        pad.r2 = 1.0;
                    }
                }
            }
        }
        pad
    }

    pub fn draw(&self, painter: &Painter, opacity: f32) {
        let alpha = |a: f32| (a * opacity * 255.0) as u8;
        let fill = Color32::from_white_alpha(alpha(0.18));
        let pressed = Color32::from_white_alpha(alpha(0.45));
        let stroke = Stroke::new(1.5 * self.scale, Color32::from_white_alpha(alpha(0.6)));
        let text = Color32::from_white_alpha(alpha(0.9));
        let held: Vec<usize> =
            self.fingers.values().filter_map(|f| if let Finger::Button(i) = f { Some(*i) } else { None }).collect();
        for (i, b) in BUTTONS.iter().enumerate() {
            let c = self.center(b);
            let r = b.radius * self.scale;
            painter.circle(c, r, if held.contains(&i) { pressed } else { fill }, stroke);
            match b.icon {
                Icon::Text(label) => {
                    painter.text(c, Align2::CENTER_CENTER, label, FontId::proportional(r * 0.8), text);
                }
                Icon::Arrow(d) => {
                    let (tip, side) = (d * r * 0.45, d.rot90() * r * 0.4);
                    let base = c - d * r * 0.25;
                    painter.add(Shape::convex_polygon(vec![c + tip, base + side, base - side], text, Stroke::NONE));
                }
                Icon::Windows => {
                    let size = vec2(r * 0.6, r * 0.45);
                    let back = Rect::from_center_size(c + vec2(r * 0.12, -r * 0.1), size);
                    let front = Rect::from_center_size(c + vec2(-r * 0.12, r * 0.1), size);
                    let line = Stroke::new(1.5 * self.scale, text);
                    painter.rect_stroke(back, 1.0, line, egui::StrokeKind::Middle);
                    painter.rect_filled(front, 1.0, Color32::from_black_alpha(alpha(0.5)));
                    painter.rect_stroke(front, 1.0, line, egui::StrokeKind::Middle);
                }
            }
        }
        let radius = STICK_RADIUS * self.scale;
        let mut stick = None;
        for f in self.fingers.values() {
            if let Finger::Stick { origin, at } = f {
                stick = Some((*origin, *at));
            }
        }
        // The stick rests at a default spot until a thumb lands.
        let (origin, at) = stick.unwrap_or_else(|| {
            let o = pos2(self.area.min.x + 130.0 * self.scale, self.area.max.y - 150.0 * self.scale);
            (o, o)
        });
        let d = at - origin;
        let knob = if d.length() > radius { origin + d.normalized() * radius } else { at };
        painter.circle(origin, radius, fill, stroke);
        painter.circle(knob, radius * 0.45, pressed, stroke);
    }
}
