//! The game without a window, for tests and measurements: the renderer
//! draws offscreen, input comes from a script.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use glue5_runtime::Game;
use glue5_runtime::audio::{CHANNELS, SAMPLE_RATE};
use glue5_runtime::input::{self, Gamepad, button};

const USAGE: &str = "\
usage: cargo run -r --example headless -- [options]

The game is GLUE5_GAME_DIR (./game under cargo). The run ends after --for,
when the game exits or asks to restart (its menu), or, with status 1, when
it stops working (an engine thread's trap).

  --for <s>             end after s seconds
  --press <s> <button>  press a pad button at s seconds (start, select, south
                        or a, east or b, west or x, north or y, l1, r1, l2, r2,
                        l3, r3, up, down, left, right)
  --key <s> <key>       press a key at s seconds (esc, enter, tab, backspace,
                        space, grave, shift, up, down, left, right, a letter
                        or a digit)
  --stdin               also take commands from standard input, one per line:
                        press <button>, key <key>, hold <key> <s>, text <text>,
                        size <w>x<h> (the back buffer, as a window resize)
  --shots <n> <dir>     every n-th presented frame to a PNG in dir
  --size <w>x<h>        back buffer size (1280x720)
  --fps <n>             frames per second at most, as the app's setting (the
                        engine's limiter; 0, the default: as fast as it can)
  --low                 the low-memory profile (smaller pools, its graphics
                        preset written into the game's settings)
  --setting <name>=<v>  a graphics setting written into the game's settings
                        (settings.xml names, e.g. ShadowQuality=3), after --low
  --sandbox             start in the sandbox (the map, no story) instead of
                        the story mode
  --tour <file> <s>     visit the map: s seconds at each stop of file (lines
                        `x y z hour weather [# name]`, z `ground`: on the
                        ground there), the camera turning; the run ends
                        after the last (use with --sandbox)
  --user-dir <dir>      saves, settings and the pipeline cache (default:
                        glue5-headless in the temporary directory)
  --audio               play the sound into a pretend device, as the app does
                        (without one the engine mixes on its own clock)
  --sync-pipelines      compile pipelines when first drawn, not skipping the
                        draws meanwhile (shots comparable between runs)
  --no-gpu              no rendering at all
  --game <dir>          the game directory, instead of GLUE5_GAME_DIR

Presses hold for 0.1 s (menus repeat a key held longer).
";

/// How long a press holds its button or key, in seconds.
const HOLD: f64 = 0.1;

/// A key press: when (s), the key, whether it is down now, for how long (s).
type Key = (f64, u8, bool, f64);

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut game_dir = std::env::var_os("GLUE5_GAME_DIR").map(PathBuf::from);
    let (mut audio, mut stdin, mut low, mut sync_pipelines, mut no_gpu) = (false, false, false, false, false);
    let (mut width, mut height) = (1280, 720);
    let mut max_fps = 0;
    let (mut presses, mut keys, mut run_for, mut shots) = (Vec::new(), Vec::new(), None, None);
    let mut game_settings: Vec<(String, String)> = Vec::new();
    let mut start_mode = 1;
    let mut tour = None;
    let mut user_dir = std::env::temp_dir().join("glue5-headless");
    let mut args = std::env::args().skip(1);
    let mut next = || args.next().context(USAGE);
    while let Ok(arg) = next() {
        match arg.as_str() {
            "--for" => run_for = Some(next()?.parse::<f64>()?),
            "--press" => {
                let at: f64 = next()?.parse()?;
                presses.push((at, parse_button(&next()?)?));
            }
            "--key" => {
                let at: f64 = next()?.parse()?;
                keys.push((at, parse_key(&next()?)?, false, HOLD));
            }
            "--stdin" => stdin = true,
            "--shots" => {
                let every: u32 = next()?.parse()?;
                shots = Some((every, PathBuf::from(next()?)));
            }
            "--size" => {
                let size = next()?;
                let (w, h) = size.split_once('x').context("--size <width>x<height>")?;
                (width, height) = (w.parse()?, h.parse()?);
            }
            "--fps" => max_fps = next()?.parse()?,
            "--low" => low = true,
            "--setting" => {
                let setting = next()?;
                let (name, value) = setting.split_once('=').context("--setting <name>=<value>")?;
                game_settings.push((name.into(), value.into()));
            }
            "--sandbox" => start_mode = 2,
            "--tour" => {
                let path = next()?;
                let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
                tour = Some((glue5_runtime::tour::parse(&text)?, Duration::from_secs_f64(next()?.parse()?)));
            }
            "--user-dir" => user_dir = PathBuf::from(next()?),
            "--audio" => audio = true,
            "--sync-pipelines" => sync_pipelines = true,
            "--no-gpu" => no_gpu = true,
            "--game" => game_dir = Some(PathBuf::from(next()?)),
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown argument {other}\n\n{USAGE}"),
        }
    }
    let game_dir = game_dir.context("no game: set GLUE5_GAME_DIR or pass --game")?;
    if low {
        glue5_runtime::game_settings::apply_low_preset(&user_dir)?;
    }
    if !game_settings.is_empty() {
        glue5_runtime::game_settings::apply(&user_dir, &game_settings)?;
    }
    let gpu = if no_gpu {
        glue5_runtime::gpu::null()
    } else {
        let mut settings = glue5_gpu::Settings::for_game(&game_dir);
        settings.cache_dir = Some(user_dir.join("cache"));
        settings.sync_pipelines = sync_pipelines;
        if let Some((every, dir)) = shots {
            std::fs::create_dir_all(&dir)?;
            settings.shot_every = every;
            settings.shot_dir = Some(dir);
        }
        glue5_gpu::backend(settings, None)
    };
    // The camera turns all along the tour, as fast as the stick turns it.
    let spin = if tour.is_some() { [1.0, 0.0] } else { [0.0; 2] };
    if let Some((stops, stay)) = tour {
        log::info!("tour: {} stops, {} s each", stops.len(), stay.as_secs_f64());
        glue5_runtime::tour::start(stops, stay);
    }
    let game = glue5_runtime::start(
        glue5_runtime::Config { game_dir, user_dir, width, height, start_mode, low_memory: low, max_fps },
        gpu,
    )?;
    if audio {
        let out = game.audio();
        std::thread::spawn(move || {
            // The app's sound device: 256 frames at a time.
            const PERIOD: usize = 256;
            let mut buf = [0f32; PERIOD * CHANNELS];
            let (start, mut frames) = (Instant::now(), 0);
            loop {
                frames += PERIOD;
                let due = start + Duration::from_secs_f64(frames as f64 / SAMPLE_RATE as f64);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                out.read(&mut buf);
            }
        });
    }
    let (live_tx, live) = std::sync::mpsc::channel::<String>();
    if stdin {
        std::thread::spawn(move || {
            for line in std::io::stdin().lines().map_while(Result::ok) {
                let _ = live_tx.send(line);
            }
        });
    }
    let t0 = Instant::now();
    let mut focused = false;
    loop {
        let t = t0.elapsed().as_secs_f64();
        for line in live.try_iter() {
            if let Err(e) = command(&line, t, &game, &mut presses, &mut keys) {
                log::warn!("{line}: {e}");
            }
        }
        if !focused && let Some(input) = game.input() {
            input.focus(true); // without focus the engine ignores input
            focused = true;
        }
        if let Some(input) = game.input() {
            for (at, vk, down, hold) in &mut keys {
                let held = (*at..*at + *hold).contains(&t);
                if held != *down {
                    input.key(*vk, held);
                    *down = held;
                }
            }
        }
        let buttons = presses.iter().filter(|(at, _)| (*at..at + HOLD).contains(&t)).fold(0, |b, (_, bits)| b | bits);
        input::set_gamepad(0, Gamepad { connected: true, buttons, right: spin, ..Default::default() });
        if run_for.is_some_and(|limit| t >= limit) {
            std::process::exit(0);
        }
        if let Some(exit) = glue5_runtime::exit::requested() {
            log::info!("the engine asked for {exit:?} at {t:.1} s");
            std::process::exit(0);
        }
        if glue5_runtime::tour::finished() {
            log::info!("tour: done at {t:.1} s");
            std::process::exit(0);
        }
        if glue5_runtime::exit::failed().is_some() {
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A line from standard input, at `t` seconds.
fn command(line: &str, t: f64, game: &Game, presses: &mut Vec<(f64, u16)>, keys: &mut Vec<Key>) -> Result<()> {
    match line.split_whitespace().collect::<Vec<_>>()[..] {
        ["press", name] => presses.push((t, parse_button(name)?)),
        ["key", name] => keys.push((t, parse_key(name)?, false, HOLD)),
        ["hold", name, secs] => keys.push((t, parse_key(name)?, false, secs.parse()?)),
        ["text", ..] => game.input().context("no input yet")?.text(line.trim_start()["text".len()..].trim()),
        ["size", size] => {
            let (w, h) = size.split_once('x').context("size <width>x<height>")?;
            game.input().context("no input yet")?.resize(w.parse()?, h.parse()?);
        }
        _ => bail!("expected `press <button>`, `key <key>`, `hold <key> <s>`, `text <text>` or `size <w>x<h>`"),
    }
    Ok(())
}

fn parse_button(name: &str) -> Result<u16> {
    Ok(match name {
        "start" => button::START,
        "select" => button::SELECT,
        "south" | "a" | "cross" => button::SOUTH,
        "east" | "b" | "circle" => button::EAST,
        "west" | "x" | "square" => button::WEST,
        "north" | "y" | "triangle" => button::NORTH,
        "l1" => button::L1,
        "r1" => button::R1,
        "l2" => button::L2,
        "r2" => button::R2,
        "l3" => button::L3,
        "r3" => button::R3,
        "up" => button::UP,
        "down" => button::DOWN,
        "left" => button::LEFT,
        "right" => button::RIGHT,
        other => bail!("unknown button {other}"),
    })
}

/// Windows virtual-key codes.
fn parse_key(name: &str) -> Result<u8> {
    Ok(match name {
        "esc" => 0x1B,
        "enter" => 0x0D,
        "tab" => 0x09,
        "backspace" => 0x08,
        "space" => 0x20,
        "grave" => 0xC0,
        "shift" => 0x10,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphanumeric() => s.as_bytes()[0].to_ascii_uppercase(),
        other => bail!("unknown key {other}"),
    })
}
