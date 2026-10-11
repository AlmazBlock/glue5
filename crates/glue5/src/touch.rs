//! Touch controls: an on-screen gamepad laid out like one held in landscape.
//! The left thumb has the movement stick and the D-pad, the right thumb the
//! camera stick (the right stick) and the face buttons: a stick appears where
//! a thumb lands on its half of the screen, away from the buttons; triggers and shoulder buttons sit in the top corners.
//! What it produces is a [`Gamepad`], so the game plays exactly as with a
//! controller (its prompts, aim assist).

use std::collections::HashMap;

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use glue5_runtime::input::{Gamepad, button};

use crate::haptics;

/// Stick travel, in points (scaled with the controls).
const STICK_RADIUS: f32 = 56.0;
/// Phones are held closer: their controls are this much smaller.
const PHONE_SCALE: f32 = 0.82;
/// Below this many points on its short side, a screen is a phone's.
const PHONE_SIDE: f32 = 500.0;

/// A position in the safe area, in points: from its left or right edge, or
/// from its middle (x), and from its top or bottom edge (y).
#[derive(Clone, Copy)]
enum At {
    LeftTop(f32, f32),
    LeftBottom(f32, f32),
    RightTop(f32, f32),
    RightBottom(f32, f32),
    MidTop(f32, f32),
}

#[derive(Clone, Copy)]
struct Button {
    icon: Icon,
    bits: u16,
    at: At,
    radius: f32,
}

#[derive(Clone, Copy)]
enum Icon {
    Text(&'static str),
    /// Two overlapping windows (Back / View).
    Windows,
}

const fn b(label: &'static str, bits: u16, at: At, radius: f32) -> Button {
    Button { icon: Icon::Text(label), bits, at, radius }
}

/// Face buttons in a diamond around this point (right thumb, above the
/// camera stick's reach), this far apart.
const FACE: (f32, f32) = (92.0, 112.0);
const FACE_SPREAD: f32 = 46.0;

const BUTTONS: &[Button] = &[
    b("A", button::SOUTH, At::RightBottom(FACE.0, FACE.1 - FACE_SPREAD), 25.0),
    b("B", button::EAST, At::RightBottom(FACE.0 - FACE_SPREAD, FACE.1), 25.0),
    b("X", button::WEST, At::RightBottom(FACE.0 + FACE_SPREAD, FACE.1), 25.0),
    b("Y", button::NORTH, At::RightBottom(FACE.0, FACE.1 + FACE_SPREAD), 25.0),
    b("RT", button::R2, At::RightTop(56.0, 46.0), 28.0),
    b("RB", button::R1, At::RightTop(56.0, 116.0), 22.0),
    b("LT", button::L2, At::LeftTop(56.0, 46.0), 28.0),
    b("LB", button::L1, At::LeftTop(56.0, 116.0), 22.0),
    b("R3", button::R3, At::RightBottom(304.0, 150.0), 18.0),
    b("L3", button::L3, At::LeftBottom(56.0, 150.0), 18.0),
    b("☰", button::START, At::MidTop(34.0, 30.0), 20.0),
    Button { icon: Icon::Windows, bits: button::SELECT, at: At::MidTop(-34.0, 30.0), radius: 20.0 },
];

/// Where the sticks rest (drawn there until a thumb takes them).
const MOVE_REST: At = At::LeftBottom(150.0, 96.0);
const LOOK_REST: At = At::RightBottom(236.0, 92.0);
/// The D-pad: its centre, the distance to each arm's centre and the arms'
/// half width.
const DPAD: At = At::LeftTop(170.0, 120.0);
const DPAD_ARM: f32 = 34.0;
const DPAD_HALF: f32 = 17.0;
/// The D-pad's directions: (bit, unit vector).
const DPAD_DIRS: [(u16, Vec2); 4] = [
    (button::UP, vec2(0.0, -1.0)),
    (button::DOWN, vec2(0.0, 1.0)),
    (button::LEFT, vec2(-1.0, 0.0)),
    (button::RIGHT, vec2(1.0, 0.0)),
];

#[derive(Clone, Copy, Debug)]
enum Finger {
    /// A stick held: where the thumb landed and where it is.
    Move {
        origin: Pos2,
        at: Pos2,
    },
    Look {
        origin: Pos2,
        at: Pos2,
    },
    /// The D-pad, pressed in this direction (a bit of `DPAD_DIRS`; the thumb
    /// can roll from one to the next).
    Dpad(u16),
    Button(usize),
}

pub struct Touch {
    fingers: HashMap<u64, Finger>,
    /// Buttons pressed since the last [`Touch::gamepad`]: a tap whose finger
    /// lifts before the gamepad is read (both in one batch of events) still
    /// presses its button once.
    pressed: u16,
    /// Points per unit of SDL's normalised finger coordinates.
    screen: Vec2,
    /// Where the controls go: clear of notches and rounded corners.
    area: Rect,
    scale: f32,
    /// A tap under each button or D-pad direction pressed.
    pub haptics: bool,
}

impl Touch {
    pub fn new() -> Self {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0));
        Touch { fingers: HashMap::new(), pressed: 0, screen: area.size(), area, scale: 1.0, haptics: false }
    }

    /// The window's size and safe area in points, and the controls' size
    /// setting.
    pub fn layout(&mut self, screen: Vec2, area: Rect, scale: f32) {
        self.screen = screen;
        self.area = area;
        let phone = screen.x.min(screen.y) < PHONE_SIDE;
        self.scale = scale * if phone { PHONE_SCALE } else { 1.0 };
    }

    fn pos(&self, at: At) -> Pos2 {
        let (s, a) = (self.scale, self.area);
        match at {
            At::LeftTop(x, y) => pos2(a.min.x + x * s, a.min.y + y * s),
            At::LeftBottom(x, y) => pos2(a.min.x + x * s, a.max.y - y * s),
            At::RightTop(x, y) => pos2(a.max.x - x * s, a.min.y + y * s),
            At::RightBottom(x, y) => pos2(a.max.x - x * s, a.max.y - y * s),
            At::MidTop(x, y) => pos2(a.center().x + x * s, a.min.y + y * s),
        }
    }

    fn stick_radius(&self) -> f32 {
        STICK_RADIUS * self.scale
    }

    fn hit(&self, b: &Button, p: Pos2) -> bool {
        // A little more generous than drawn.
        self.pos(b.at).distance(p) <= b.radius * self.scale * 1.25
    }

    /// The D-pad direction under `p`, if it is on the D-pad.
    fn dpad_at(&self, p: Pos2) -> Option<u16> {
        let d = p - self.pos(DPAD);
        let reach = (DPAD_ARM + DPAD_HALF) * self.scale * 1.2;
        if d.length() > reach {
            return None;
        }
        // The arm the thumb leans towards; its centre counts as up.
        let dir = if d.x.abs() > d.y.abs() { vec2(d.x.signum(), 0.0) } else { vec2(0.0, d.y.signum()) };
        DPAD_DIRS.iter().find(|(_, v)| *v == dir).map(|(bit, _)| *bit).or(Some(button::UP))
    }

    pub fn finger_down(&mut self, id: u64, x: f32, y: f32) {
        let p = pos2(x * self.screen.x, y * self.screen.y);
        let finger = if let Some(i) = BUTTONS.iter().position(|b| self.hit(b, p)) {
            Finger::Button(i)
        } else if let Some(dir) = self.dpad_at(p) {
            Finger::Dpad(dir)
        } else if p.x < self.screen.x / 2.0 {
            Finger::Move { origin: p, at: p }
        } else {
            Finger::Look { origin: p, at: p }
        };
        self.pressed |= match finger {
            Finger::Button(i) => BUTTONS[i].bits,
            Finger::Dpad(bit) => bit,
            _ => 0,
        };
        if self.haptics && matches!(finger, Finger::Button(_) | Finger::Dpad(_)) {
            haptics::tap();
        }
        self.fingers.insert(id, finger);
    }

    pub fn finger_motion(&mut self, id: u64, x: f32, y: f32) {
        let p = pos2(x * self.screen.x, y * self.screen.y);
        let dpad = self.dpad_at(p);
        match self.fingers.get_mut(&id) {
            Some(Finger::Move { at, .. } | Finger::Look { at, .. }) => *at = p,
            Some(Finger::Dpad(dir)) => {
                if let Some(d) = dpad {
                    // Rolling onto another arm is a new press.
                    if d != *dir && self.haptics {
                        haptics::tap();
                    }
                    *dir = d;
                    self.pressed |= d;
                }
            }
            _ => {}
        }
    }

    pub fn finger_up(&mut self, id: u64) {
        self.fingers.remove(&id);
    }

    pub fn clear(&mut self) {
        self.fingers.clear();
        self.pressed = 0;
    }

    /// The gamepad the fingers make now.
    pub fn gamepad(&mut self) -> Gamepad {
        let mut pad = Gamepad { connected: true, buttons: std::mem::take(&mut self.pressed), ..Default::default() };
        let radius = self.stick_radius();
        let stick = |origin: Pos2, at: Pos2| stick_value((at - origin) / radius);
        for finger in self.fingers.values() {
            match *finger {
                Finger::Move { origin, at } => pad.left = stick(origin, at),
                Finger::Look { origin, at } => pad.right = stick(origin, at),
                Finger::Dpad(bit) => pad.buttons |= bit,
                Finger::Button(i) => {
                    let bits = BUTTONS[i].bits;
                    pad.buttons |= bits;
                    if bits == button::L2 {
                        pad.l2 = 1.0;
                    }
                    if bits == button::R2 {
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
        let held_button = |i: usize| self.fingers.values().any(|f| matches!(f, Finger::Button(j) if *j == i));
        for (i, b) in BUTTONS.iter().enumerate() {
            let c = self.pos(b.at);
            let r = b.radius * self.scale;
            painter.circle(c, r, if held_button(i) { pressed } else { fill }, stroke);
            match b.icon {
                Icon::Text(label) => {
                    painter.text(c, Align2::CENTER_CENTER, label, FontId::proportional(r * 0.8), text);
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

        // Four separate arms keep the translucent fill and outlines from
        // overlapping at the centre.
        let c = self.pos(DPAD);
        let (arm, half) = (DPAD_ARM * self.scale, DPAD_HALF * self.scale);
        let round = CornerRadius::same((half * 0.5) as u8);
        let held_dir = self.fingers.values().find_map(|f| if let Finger::Dpad(bit) = f { Some(*bit) } else { None });
        for (bit, d) in DPAD_DIRS {
            let centre = c + d * arm;
            painter.rect(
                Rect::from_center_size(centre, Vec2::splat(2.0 * half)),
                round,
                if held_dir == Some(bit) { pressed } else { fill },
                stroke,
                egui::StrokeKind::Middle,
            );
            let (tip, side) = (d * half * 0.6, d.rot90() * half * 0.55);
            let base = centre - d * half * 0.35;
            painter.add(Shape::convex_polygon(vec![centre + tip, base + side, base - side], text, Stroke::NONE));
        }

        // The sticks, where held or at rest.
        let radius = self.stick_radius();
        let held = |look: bool| {
            self.fingers.values().find_map(|f| match *f {
                Finger::Move { origin, at } if !look => Some((origin, at)),
                Finger::Look { origin, at } if look => Some((origin, at)),
                _ => None,
            })
        };
        for (look, rest) in [(false, MOVE_REST), (true, LOOK_REST)] {
            let rest = self.pos(rest);
            let (origin, at) = held(look).unwrap_or((rest, rest));
            let d = at - origin;
            let knob = if d.length() > radius { origin + d.normalized() * radius } else { at };
            painter.circle(origin, radius, fill, stroke);
            painter.circle(knob, radius * 0.45, pressed, stroke);
        }
    }
}

/// A stick's value for a thumb this far from where it landed (in stick
/// radii): the circle of its reach stretched onto the square, so that the
/// axis the thumb leans along reads as far as the thumb went. A thumb sliding
/// sideways drifts up or down as well; on the circle, 30 degrees of drift
/// left the steering at 0.87 at full reach.
fn stick_value(v: Vec2) -> [f32; 2] {
    let reach = v.length().min(1.0);
    let lean = v.x.abs().max(v.y.abs());
    if lean == 0.0 {
        return [0.0, 0.0];
    }
    let v = v * (reach / lean);
    [v.x, v.y]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An iPhone 13 Pro in landscape: 844 x 390 points, the notch's inset on
    /// the left, the home indicator's at the bottom.
    fn phone() -> Touch {
        let mut t = Touch::new();
        let screen = vec2(844.0, 390.0);
        t.layout(screen, Rect::from_min_max(pos2(47.0, 0.0), pos2(797.0, 369.0)), 1.0);
        t
    }

    fn circles(t: &Touch) -> Vec<(&'static str, Pos2, f32)> {
        let mut all: Vec<_> = BUTTONS
            .iter()
            .map(|b| (if let Icon::Text(l) = b.icon { l } else { "View" }, t.pos(b.at), b.radius * t.scale))
            .collect();
        all.push(("D-pad", t.pos(DPAD), (DPAD_ARM + DPAD_HALF) * t.scale * std::f32::consts::SQRT_2));
        all.push(("move", t.pos(MOVE_REST), t.stick_radius()));
        all.push(("look", t.pos(LOOK_REST), t.stick_radius()));
        // The app's settings button (ui.rs), at the size setting's scale.
        all.push(("gear", pos2(t.area.max.x - 156.0, t.area.min.y + 40.0), 20.0));
        all
    }

    #[test]
    fn controls_fit_a_phone_without_overlapping() {
        let t = phone();
        let all = circles(&t);
        for (i, (a, pa, ra)) in all.iter().enumerate() {
            let r = Rect::from_center_size(*pa, Vec2::splat(2.0 * ra));
            assert!(t.area.expand(1.0).contains_rect(r), "{a} leaves the safe area");
            for (b, pb, rb) in &all[i + 1..] {
                assert!(pa.distance(*pb) > ra + rb, "{a} overlaps {b}");
            }
        }
    }

    #[test]
    fn thumbs_take_the_controls_under_them() {
        let mut t = phone();
        let at = |t: &Touch, p: Pos2| (p.x / t.screen.x, p.y / t.screen.y);
        // The camera stick, from near its resting place: the right stick.
        let look = t.pos(LOOK_REST) + vec2(10.0, 0.0);
        let (x, y) = at(&t, look);
        t.finger_down(1, x, y);
        let (x, y) = at(&t, look + vec2(2.0 * t.stick_radius(), 0.0));
        t.finger_motion(1, x, y);
        assert_eq!(t.gamepad().right, [1.0, 0.0]);
        // The D-pad's left arm, rolled to its down arm.
        let left = t.pos(DPAD) + vec2(-DPAD_ARM * t.scale, 0.0);
        let (x, y) = at(&t, left);
        t.finger_down(2, x, y);
        assert_eq!(t.gamepad().buttons & button::LEFT, button::LEFT);
        let (x, y) = at(&t, t.pos(DPAD) + vec2(0.0, DPAD_ARM * t.scale));
        t.finger_motion(2, x, y);
        assert_eq!(t.gamepad().buttons & (button::LEFT | button::DOWN), button::DOWN);
        // A tap that lifts before the gamepad is read still presses once.
        t.clear();
        let a = BUTTONS.iter().position(|b| b.bits == button::SOUTH).unwrap();
        let p = t.pos(BUTTONS[a].at);
        let (x, y) = at(&t, p);
        t.finger_down(4, x, y);
        t.finger_up(4);
        assert_eq!(t.gamepad().buttons, button::SOUTH);
        assert_eq!(t.gamepad().buttons, 0);
        // Anywhere on the right half away from the buttons: the camera stick,
        // from where the thumb lands (not a drag: held still, it stays put).
        t.clear();
        t.finger_down(3, 0.6, 0.5);
        let (x, y) = (0.6 + t.stick_radius() / t.screen.x * 0.5, 0.5);
        t.finger_motion(3, x, y);
        assert!((t.gamepad().right[0] - 0.5).abs() < 1e-3);
        t.finger_motion(3, x, y);
        assert!((t.gamepad().right[0] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_thumb_drifting_off_the_axis_keeps_its_reach() {
        let at = |deg: f32, reach: f32| stick_value(Vec2::angled(deg.to_radians()) * reach);
        let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        // Sideways and 30 degrees up, all the way: full steering.
        assert!(near(at(-150.0, 2.0), [-1.0, -(30f32.to_radians().tan())]));
        // Half way: half.
        assert!((at(-30.0, 0.5)[0] - 0.5).abs() < 1e-4);
        assert!(near(at(90.0, 1.0), [0.0, 1.0]));
        assert!(near(at(45.0, 1.0), [1.0, 1.0]));
        assert_eq!(stick_value(Vec2::ZERO), [0.0, 0.0]);
    }
}
