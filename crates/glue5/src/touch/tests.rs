use super::*;

/// An iPhone 13 Pro in landscape: 844 x 390 points, the notch's inset on
/// the left, the home indicator's at the bottom.
fn phone() -> Touch {
    let mut t = Touch::new();
    let screen = vec2(844.0, 390.0);
    t.layout(screen, Rect::from_min_max(pos2(47.0, 0.0), pos2(797.0, 369.0)), 1.0);
    t
}

/// The screens the layouts must fit: an iPhone 16 Pro, an iPhone 13 Pro, an
/// iPad Pro 11" (points, and safe insets left, top, right, bottom).
const SCREENS: [(Vec2, [f32; 4]); 3] = [
    (vec2(874.0, 402.0), [62.0, 0.0, 62.0, 21.0]),
    (vec2(844.0, 390.0), [47.0, 0.0, 47.0, 21.0]),
    (vec2(1194.0, 834.0), [0.0, 24.0, 0.0, 20.0]),
];

fn at(t: &Touch, p: Pos2) -> (f32, f32) {
    (p.x / t.screen.x, p.y / t.screen.y)
}

fn context(activity: Activity) -> PlayerContext {
    PlayerContext {
        activity,
        vehicle_near: true,
        phone: false,
        menu: false,
        cutscene: false,
        scripted: false,
        character: Character::Franklin,
        gun: false,
    }
}

/// Where the layout's button labelled `label` is now.
fn spot(t: &Touch, label: &str) -> Pos2 {
    t.buttons()
        .iter()
        .find(|b| matches!(b.key, Key::Action(a) if unsafe { (*a).label } == label))
        .map(|b| b.at)
        .unwrap_or_else(|| panic!("no {label}"))
}

const ACTIVITIES: [Activity; 8] = [
    Activity::OnFoot,
    Activity::Driving(Driving::Car),
    Activity::Driving(Driving::Bicycle),
    Activity::Driving(Driving::Heli),
    Activity::Driving(Driving::Plane),
    Activity::Driving(Driving::Submarine),
    Activity::Passenger,
    Activity::Parachute,
];

/// Where the phone shows in the game at its usual place, on a screen this
/// wide over high (as `glue5_runtime::input::phone_rect` has it).
fn phone_at(aspect: f32) -> [f32; 4] {
    let depth = 113.0 * 35f32.to_radians().tan();
    let u = 0.5 + 0.5 * 93.05 / (depth * aspect.min(16.0 / 9.0));
    let v = 0.5 + 0.5 * 51.7 / depth;
    let (w, h) = (0.12 / aspect, 0.2417);
    [u - w, v - h, u + w, v + h]
}

#[test]
fn locked_on_a_drag_pushes_the_right_stick() {
    let mut t = phone();
    t.set_context(context(Activity::OnFoot));
    // Aim held, its thumb trembling: no turn, a stick in the dead zone.
    let p = spot(&t, "Aim");
    let (x, y) = at(&t, p);
    t.finger_down(1, x, y);
    t.aim_locked = true;
    let (x, y) = at(&t, p + vec2(1.0, 0.0));
    t.finger_motion(1, x, y);
    let pad = t.gamepad();
    assert!(pad.right[0] < 0.2 && pad.l2 == 1.0);
    assert_eq!(t.take_look(), (0, 0));
    // A swipe on the camera: the stick all the way, then easing off.
    let c = pos2(560.0, 160.0);
    let (x, y) = at(&t, c);
    t.finger_down(2, x, y);
    let (x, y) = at(&t, c + vec2(-2.0 * LOCKED_SWIPE, 0.0));
    t.finger_motion(2, x, y);
    assert_eq!(t.gamepad().right, [-1.0, 0.0]);
    assert_eq!(t.gamepad().right, [-1.0, 0.0]);
    assert!(t.gamepad().right[0] > -1.0);
    assert_eq!(t.take_look(), (0, 0));
    // Unlocked, drags turn the camera again.
    t.aim_locked = false;
    let (x, y) = at(&t, c);
    t.finger_motion(2, x, y);
    assert_eq!(t.gamepad().right, [0.0, 0.0]);
    assert_ne!(t.take_look(), (0, 0));
}

#[test]
fn a_tap_lifting_before_the_pad_is_read_presses_once() {
    let mut t = phone();
    t.set_context(context(Activity::OnFoot));
    let (x, y) = at(&t, spot(&t, "Jump"));
    t.finger_down(4, x, y);
    t.finger_up(4);
    assert_eq!(t.gamepad().buttons, button::WEST);
    assert_eq!(t.gamepad().buttons, 0);
}

#[test]
fn touch_layouts_fit_without_overlapping() {
    for (screen, [l, top, r, b]) in SCREENS {
        let mut t = Touch::new();
        let safe = Rect::from_min_max(pos2(l, top), pos2(screen.x - r, screen.y - b));
        t.layout(screen, safe, 1.0);
        let cases = ACTIVITIES.into_iter().flat_map(|a| [(a, false, false, false), (a, true, false, false)]);
        let extra = [(Activity::OnFoot, false, true, false), (Activity::OnFoot, false, false, true)];
        for (activity, phone, cutscene, scripted) in cases.chain(extra) {
            t.set_context(PlayerContext { phone, cutscene, scripted, ..context(activity) });
            t.set_game(Vec::new(), phone.then(|| phone_at(screen.x / screen.y)));
            let what = format!("{screen:?} {activity:?} phone {phone} cutscene {cutscene} scripted {scripted}");
            let ring = if t.sprints() { SPRINT_REACH } else { 1.0 };
            let mut all: Vec<(String, Pos2, f32)> =
                t.buttons().iter().map(|b| (format!("{:?}", b.key), b.at, b.radius)).collect();
            if t.has_stick() {
                all.push(("stick".into(), t.pos(MOVE_REST), ring * t.stick_radius()));
            }
            // The app's settings button, beside pause and the view.
            all.push(("settings".into(), crate::ui::gear_center(safe, t.scale), crate::ui::GEAR_RADIUS * t.scale));
            for (i, (a, pa, ra)) in all.iter().enumerate() {
                let r = Rect::from_center_size(*pa, Vec2::splat(2.0 * ra));
                assert!(t.area.expand(1.0).contains_rect(r), "{what}: {a} leaves the safe area");
                for (b, pb, rb) in &all[i + 1..] {
                    assert!(pa.distance(*pb) > ra + rb, "{what}: {a} overlaps {b}");
                }
            }
        }
    }
}

#[test]
fn the_layout_follows_the_player() {
    let mut t = phone();
    let layout = |t: &Touch| t.touch_layout().map(|l| l[0].label);
    t.set_context(context(Activity::OnFoot));
    assert_eq!(layout(&t), Some("Fire"));
    t.set_context(context(Activity::Driving(Driving::Bicycle)));
    assert_eq!(layout(&t), Some("Pedal"));
    t.set_context(context(Activity::Parachute));
    assert_eq!(layout(&t), Some("Open"));
    t.set_context(PlayerContext { cutscene: true, ..context(Activity::Passenger) });
    assert_eq!(layout(&t), Some("Skip"));
    assert!(!t.has_stick());
    // A script's menu: the D-pad, Select, Back.
    t.set_context(PlayerContext { scripted: true, ..context(Activity::OnFoot) });
    assert_eq!(layout(&t), Some("▲"));
    let bits: Vec<u16> = t.buttons().iter().map(|b| b.bits).collect();
    assert!(bits.contains(&button::SOUTH) && bits.contains(&button::EAST) && bits.contains(&button::LEFT));
    // The phone's keys join the layout, the buttons the phone uses go.
    t.set_context(PlayerContext { phone: true, ..context(Activity::OnFoot) });
    let bits: Vec<u16> = t.buttons().iter().map(|b| b.bits).collect();
    for b in [button::UP, button::DOWN, button::LEFT, button::RIGHT, button::SOUTH, button::EAST] {
        assert!(bits.contains(&b), "no phone key {b:#x}");
    }
    for b in [button::WEST, button::NORTH, button::R2, button::L1] {
        assert!(!bits.contains(&b), "{b:#x} still shown with the phone out");
    }
    // In a car the pedals stay.
    t.set_context(PlayerContext { phone: true, ..context(Activity::Driving(Driving::Car)) });
    let bits: Vec<u16> = t.buttons().iter().map(|b| b.bits).collect();
    assert!(bits.contains(&button::R2) && bits.contains(&button::L2) && !bits.contains(&button::NORTH));
    // Enter only next to a vehicle.
    t.set_context(PlayerContext { vehicle_near: false, ..context(Activity::OnFoot) });
    assert!(t.buttons().iter().all(|b| b.bits != button::NORTH));
    // The pause menu: no layout, nor pad.
    t.set_context(PlayerContext { menu: true, ..context(Activity::OnFoot) });
    assert_eq!(layout(&t), None);
    assert_eq!(t.gamepad().buttons, 0);
}

#[test]
fn the_game_asks_and_a_button_shows_it() {
    let mut t = phone();
    t.set_context(PlayerContext { scripted: true, ..context(Activity::OnFoot) });
    // The character creator's prompts: Quit (B), Random (X), Turn Head.
    let prompts = vec![
        Prompt { pads: vec![Pad::Buttons(button::EAST)], input: Some(202), label: "Quit".into() },
        Prompt { pads: vec![Pad::Buttons(button::WEST)], input: Some(203), label: "Random".into() },
        Prompt { pads: vec![Pad::Stick { right: true, x: true, y: true }], input: None, label: "Turn Head".into() },
    ];
    t.set_game(prompts, None);
    let buttons = t.buttons();
    // Back does Quit: ringed; Random has a button of its own, pressed as X.
    assert!(buttons.iter().find(|b| b.bits == button::EAST).unwrap().asked);
    let own: Vec<&Button> = buttons.iter().filter(|b| matches!(b.key, Key::Prompt { .. })).collect();
    assert_eq!(own.len(), 1, "the stick is no button");
    let random = own[0];
    assert_eq!((random.icon, random.bits), ("pad-x", button::WEST));
    let (x, y) = at(&t, random.at);
    t.finger_down(1, x, y);
    assert_eq!(t.gamepad().buttons, button::WEST);
    t.finger_up(1);
    // A drag on the right is the right stick.
    let c = pos2(500.0, 120.0);
    let (x, y) = at(&t, c);
    t.finger_down(2, x, y);
    let (x, y) = at(&t, c + vec2(1.5 * t.stick_radius(), 0.0));
    t.finger_motion(2, x, y);
    assert_eq!(t.gamepad().right, [1.0, 0.0]);
    assert_eq!(t.take_look(), (0, 0));
}

#[test]
fn a_prompt_on_foot_lights_the_button_that_does_it() {
    let mut t = phone();
    t.set_context(context(Activity::OnFoot));
    // "Press D-pad right to talk": Action does it, ringed; nothing added.
    t.set_game(vec![Prompt { pads: vec![Pad::Buttons(button::RIGHT)], input: Some(51), label: String::new() }], None);
    let buttons = t.buttons();
    assert!(buttons.iter().find(|b| b.bits == button::RIGHT).unwrap().asked);
    assert!(buttons.iter().all(|b| !matches!(b.key, Key::Prompt { .. })));
    // "Press Y to get up", no vehicle near (Enter hidden): a pill, as Y.
    t.set_context(PlayerContext { vehicle_near: false, ..context(Activity::OnFoot) });
    t.set_game(vec![Prompt { pads: vec![Pad::Buttons(button::NORTH)], input: Some(23), label: String::new() }], None);
    let own = t.buttons().into_iter().find(|b| matches!(b.key, Key::Prompt { .. })).unwrap();
    assert_eq!((own.bits, own.input, own.icon), (button::NORTH, None, "pad-y"));
    let occupied = t.occupied();
    assert!(occupied.iter().filter(|r| r.intersects(own.rect())).count() == 1, "it is on a control");
}

#[test]
fn an_input_only_prompt_presses_the_input() {
    let mut t = phone();
    t.set_context(context(Activity::Driving(Driving::Car)));
    // A script's scene waits for its skip: a button, the input pressed.
    t.set_game(vec![Prompt { pads: Vec::new(), input: Some(SKIP_CUTSCENE), label: String::new() }], None);
    let skip = t.buttons().into_iter().find(|b| matches!(b.key, Key::Prompt { .. })).unwrap();
    assert_eq!(skip.icon, "skip");
    let (x, y) = at(&t, skip.at);
    t.finger_down(1, x, y);
    assert_eq!(t.take_inputs(), vec![SKIP_CUTSCENE]);
    assert_eq!(t.gamepad().buttons, 0);
    t.finger_up(1);
    assert!(t.take_inputs().is_empty());
    // The cutscene's own skip presses it too.
    t.set_game(Vec::new(), None);
    t.set_context(PlayerContext { cutscene: true, ..context(Activity::OnFoot) });
    let (x, y) = at(&t, spot(&t, "Skip"));
    t.finger_down(2, x, y);
    assert_eq!(t.take_inputs(), vec![SKIP_CUTSCENE]);
}

#[test]
fn in_the_pause_menu_fingers_are_the_mouse() {
    let mut t = phone();
    t.set_context(PlayerContext { menu: true, ..context(Activity::OnFoot) });
    // A tap: the pointer there, a click that lasts until the engine sees it.
    let (x, y) = at(&t, pos2(400.0, 200.0));
    t.finger_down(1, x, y);
    t.finger_up(1);
    let m = t.take_mouse();
    assert!(!m.down && m.keys == [false; 6], "the pointer moves first");
    assert!((m.at.unwrap().x - x).abs() < 1e-6);
    std::thread::sleep(CLICK_DELAY);
    assert!(t.take_mouse().down);
    // A drag moves it, button down; a second finger pinches: the wheel.
    t.finger_down(1, x, y);
    let (x2, y2) = at(&t, pos2(450.0, 220.0));
    t.finger_motion(1, x2, y2);
    assert!((t.take_mouse().at.unwrap().x - x2).abs() < 1e-6);
    let (px, py) = at(&t, pos2(450.0, 320.0));
    t.finger_down(2, px, py);
    let (px, py) = at(&t, pos2(450.0, 320.0 + 2.0 * PINCH_NOTCH));
    t.finger_motion(2, px, py);
    let m = t.take_mouse();
    assert_eq!((m.wheel, m.down), (2, false));
    // Two fingers dragged up together: the wheel scrolls down.
    t.clear();
    let (ax, ay) = at(&t, pos2(400.0, 300.0));
    t.finger_down(1, ax, ay);
    let (bx, by) = at(&t, pos2(500.0, 300.0));
    t.finger_down(2, bx, by);
    let up = 3.0 * SCROLL_NOTCH + GESTURE_CLEAR;
    for (id, x) in [(1, 400.0), (2, 500.0)] {
        for step in 1..=4 {
            let (x, y) = at(&t, pos2(x, 300.0 - up * step as f32 / 4.0));
            t.finger_motion(id, x, y);
        }
    }
    let m = t.take_mouse();
    assert_eq!((m.wheel, m.down), (-3, false));
    // The keys: held while the finger is, and a tap's long enough for the
    // engine to see.
    for (i, (key, a)) in MENU_KEYS.iter().enumerate() {
        t.clear();
        let (x, y) = at(&t, t.pos(a.at));
        t.finger_down(3, x, y);
        t.finger_up(3);
        let m = t.take_mouse();
        assert!(m.keys[i] && m.keys.iter().filter(|k| **k).count() == 1 && !m.down, "{key:?}");
        std::thread::sleep(MIN_PRESS);
        assert!(!t.take_mouse().keys[i]);
    }
}

#[test]
fn the_pause_menus_keys_keep_off_the_menu() {
    for (screen, [l, top, r, b]) in SCREENS {
        let mut t = Touch::new();
        let safe = Rect::from_min_max(pos2(l, top), pos2(screen.x - r, screen.y - b));
        t.layout(screen, safe, 1.0);
        // The menu: the middle 16:9 of the screen, less its margins (its
        // right edge at 0.778 of an iPhone's width).
        let width = screen.x.min(screen.y * 16.0 / 9.0);
        let right = screen.x / 2.0 + width * (0.778 - 0.5) * (874.0 / 715.0);
        for (key, a) in MENU_KEYS.iter().skip(1) {
            let c = t.pos(a.at);
            assert!(c.x - a.radius * t.scale >= right, "{screen:?} {key:?} on the menu");
            assert!(safe.contains_rect(Rect::from_center_size(c, Vec2::splat(2.0 * a.radius * t.scale))));
        }
    }
}

#[test]
fn with_the_phone_out_the_stick_does_not_sprint() {
    let mut t = phone();
    t.set_context(PlayerContext { phone: true, ..context(Activity::OnFoot) });
    let o = pos2(200.0, 250.0);
    let (x, y) = at(&t, o);
    t.finger_down(1, x, y);
    let (x, y) = at(&t, o + vec2(0.0, -2.0 * t.stick_radius()));
    t.finger_motion(1, x, y);
    let pad = t.gamepad();
    assert_eq!((pad.left, pad.buttons & button::SOUTH), ([0.0, -1.0], 0));
}

#[test]
fn the_phone_keys_keep_off_the_phone() {
    for (screen, [l, top, r, b]) in SCREENS {
        let mut t = Touch::new();
        let safe = Rect::from_min_max(pos2(l, top), pos2(screen.x - r, screen.y - b));
        t.layout(screen, safe, 1.0);
        for activity in [Activity::OnFoot, Activity::Driving(Driving::Car)] {
            t.set_context(PlayerContext { phone: true, ..context(activity) });
            t.set_game(Vec::new(), Some(phone_at(screen.x / screen.y)));
            let phone = t.phone.unwrap();
            for key in t.buttons().iter().filter(|b| matches!(b.key, Key::Phone(_))) {
                let what = format!("{screen:?} {activity:?} {:?}", key.key);
                assert!(!key.rect().intersects(phone), "{what} on the phone {phone:?}");
                assert!(t.area.contains_rect(key.rect()), "{what} off the screen");
            }
        }
    }
}

#[test]
fn on_foot_fingers_walk_sprint_and_turn_the_camera() {
    let mut t = phone();
    t.set_context(context(Activity::OnFoot));
    let r = t.stick_radius();
    // The stick where the left thumb lands; past its ring, a sprint.
    let o = pos2(200.0, 250.0);
    let (x, y) = at(&t, o);
    t.finger_down(1, x, y);
    let (x, y) = at(&t, o + vec2(0.0, -0.5 * r));
    t.finger_motion(1, x, y);
    let pad = t.gamepad();
    assert!((pad.left[1] + 0.5).abs() < 1e-3 && pad.buttons & button::SOUTH == 0);
    let (x, y) = at(&t, o + vec2(0.0, -1.5 * r));
    t.finger_motion(1, x, y);
    let pad = t.gamepad();
    assert_eq!((pad.left, pad.buttons & button::SOUTH), ([0.0, -1.0], button::SOUTH));
    // Dragging on the right turns the camera by as much, nothing else.
    let c = pos2(560.0, 160.0);
    let (x, y) = at(&t, c);
    t.finger_down(2, x, y);
    let (x, y) = at(&t, c + vec2(10.0, -5.0));
    t.finger_motion(2, x, y);
    assert_eq!(t.take_look(), ((10.0 * LOOK_GAIN) as i32, (-5.0 * LOOK_GAIN) as i32));
    assert_eq!(t.take_look(), (0, 0));
    assert_eq!(t.gamepad().right, [0.0, 0.0]);
    // Fire held fires and turns the camera.
    let p = spot(&t, "Fire");
    let (x, y) = at(&t, p);
    t.finger_down(3, x, y);
    let (x, y) = at(&t, p + vec2(-4.0, 0.0));
    t.finger_motion(3, x, y);
    assert_eq!(t.gamepad().buttons & button::R2, button::R2);
    assert_eq!(t.take_look(), ((-4.0 * LOOK_GAIN) as i32, 0));
}

#[test]
fn a_wheel_is_picked_by_dragging_from_its_button() {
    let mut t = phone();
    t.set_context(context(Activity::OnFoot));
    let p = spot(&t, "Weapons");
    let (x, y) = at(&t, p);
    t.finger_down(1, x, y);
    let (x, y) = at(&t, p + vec2(2.0 * t.stick_radius(), 0.0));
    t.finger_motion(1, x, y);
    let pad = t.gamepad();
    assert_eq!((pad.buttons & button::L1, pad.right), (button::L1, [1.0, 0.0]));
    assert_eq!(t.take_look(), (0, 0));
}

#[test]
fn an_open_wheel_picks_with_the_other_thumb() {
    let mut t = phone();
    t.set_context(context(Activity::Driving(Driving::Car)));
    // A thumb already on the camera, then the radio held with the other.
    let c = pos2(560.0, 160.0);
    let (x, y) = at(&t, c);
    t.finger_down(1, x, y);
    let (x, y) = at(&t, c + vec2(5.0, 0.0));
    t.finger_motion(1, x, y);
    let (x, y) = at(&t, spot(&t, "Radio"));
    t.finger_down(2, x, y);
    t.take_look();
    // Its drag picks from where it was, the camera stays.
    let (x, y) = at(&t, c + vec2(5.0, -2.0 * t.stick_radius()));
    t.finger_motion(1, x, y);
    let pad = t.gamepad();
    assert_eq!((pad.buttons & button::LEFT, pad.right), (button::LEFT, [0.0, -1.0]));
    assert_eq!(t.take_look(), (0, 0));
    // The radio let go: the camera again.
    t.finger_up(2);
    let (x, y) = at(&t, c + vec2(15.0, -2.0 * t.stick_radius()));
    t.finger_motion(1, x, y);
    assert_eq!(t.gamepad().right, [0.0, 0.0]);
    assert_eq!(t.take_look(), ((10.0 * LOOK_GAIN) as i32, 0));
}

#[test]
fn an_open_wheel_picks_with_the_thumb_on_the_gas() {
    let mut t = phone();
    t.set_context(context(Activity::Driving(Driving::Car)));
    let g = spot(&t, "Gas");
    let (x, y) = at(&t, g);
    t.finger_down(1, x, y);
    let (x, y) = at(&t, g + vec2(3.0, 0.0));
    t.finger_motion(1, x, y);
    let (x, y) = at(&t, spot(&t, "Radio"));
    t.finger_down(2, x, y);
    // The thumb on the gas picks from where it was, still pressing it.
    let (x, y) = at(&t, g + vec2(3.0 - 2.0 * t.stick_radius(), 0.0));
    t.finger_motion(1, x, y);
    let pad = t.gamepad();
    assert_eq!(pad.buttons & (button::LEFT | button::R2), button::LEFT | button::R2);
    assert_eq!(pad.right, [-1.0, 0.0]);
    assert_eq!(t.take_look(), (0, 0));
    // The radio let go: the gas only.
    t.finger_up(2);
    assert_eq!(t.gamepad().right, [0.0, 0.0]);
}

#[test]
fn driving_has_its_pedals() {
    let mut t = phone();
    t.set_context(context(Activity::Driving(Driving::Car)));
    for (id, label) in [(1, "Gas"), (2, "Brake")] {
        let (x, y) = at(&t, spot(&t, label));
        t.finger_down(id, x, y);
    }
    let pad = t.gamepad();
    assert_eq!((pad.r2, pad.l2), (1.0, 1.0));
    // Steering, without a sprint.
    let (x, y) = at(&t, pos2(200.0, 250.0));
    t.finger_down(3, x, y);
    let (x, y) = at(&t, pos2(200.0 - 2.0 * t.stick_radius(), 250.0));
    t.finger_motion(3, x, y);
    let pad = t.gamepad();
    assert_eq!((pad.left, pad.buttons & button::SOUTH), ([-1.0, 0.0], 0));
}

#[test]
fn every_icon_loads() {
    let ctx = egui::Context::default();
    let icons = load_icons(&ctx);
    assert_eq!(icons.len(), ICON_FILES.len(), "an icon does not decode");
    let layouts = [ON_FOOT, CAR, BICYCLE, HELI, PLANE, SUBMARINE, PASSENGER, PARACHUTE, CUTSCENE, SCRIPTED, &PHONE_KEYS];
    let names = layouts.iter().flat_map(|l| l.iter().flat_map(|a| a.icon.split('+')));
    let pads = (0..16).map(|i| pad_icon(1 << i, None)).chain([pad_icon(0, Some(SKIP_CUTSCENE))]);
    for name in names.chain(MENU_KEYS.iter().map(|(_, a)| a.icon)).chain(pads).chain(["reload", "settings"]) {
        assert!(icons.contains_key(name), "no icon {name}");
    }
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
