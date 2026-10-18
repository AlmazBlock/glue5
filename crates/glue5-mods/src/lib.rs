//! Mods: Luau scripts, each in a script thread of the game's own, calling
//! the game's natives.
//!
//! A mod is a folder in the mods folder with a `main.luau` and, optionally,
//! a `mod.json` (`{"autostart": true}`). The application calls [`install`]
//! before the game starts, then [`start`], [`stop`], [`mods`] and
//! [`take_log`] from any thread; the mods themselves run on the engine's
//! script update (engine.rs).

mod engine;
pub mod natives;
mod vm;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModState {
    Stopped,
    Running,
    Error,
}

/// A mod, as the application shows it.
#[derive(Clone, Debug)]
pub struct Mod {
    pub name: String,
    pub state: ModState,
    /// Why it stopped, when it failed.
    pub error: Option<String>,
    /// Time it takes per frame (smoothed), in milliseconds.
    pub cpu_ms: f32,
    /// Memory of its VM, in bytes.
    pub memory: usize,
    /// It starts with the game.
    pub autostart: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub time: SystemTime,
    pub module: String,
    pub level: LogLevel,
    pub text: String,
}

enum Command {
    Start(String),
    Stop(String),
}

#[derive(Default)]
struct State {
    mods: Vec<Mod>,
    commands: Vec<Command>,
    log: VecDeque<LogLine>,
}

struct Shared {
    dir: OnceLock<PathBuf>,
    state: Mutex<State>,
    /// Commands are waiting for the engine.
    pending: AtomicBool,
}

static SHARED: Shared =
    Shared { dir: OnceLock::new(), state: Mutex::new(State { mods: Vec::new(), commands: Vec::new(), log: VecDeque::new() }), pending: AtomicBool::new(false) };

const LOG_LINES: usize = 1000;

/// Sets mods up for the game about to start: `dir` is the mods folder
/// (made if missing). The mods marked autostart start once the game runs
/// scripts. Call before `glue5_runtime::start`.
pub fn install(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    if SHARED.dir.set(dir.to_path_buf()).is_err() {
        return;
    }
    glue5_runtime::add_hooks(engine::hooks);
    log::info!("mods: {} ({})", dir.display(), natives::SUMMARY);
    refresh();
    let autostart: Vec<String> = mods().into_iter().filter(|m| m.autostart).map(|m| m.name).collect();
    for name in autostart {
        start(&name);
    }
}

/// Reads the mods folder again (new or removed mods).
pub fn refresh() {
    let Some(dir) = SHARED.dir.get() else { return };
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("main.luau").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let autostart = std::fs::read(path.join("mod.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                    .and_then(|v| v["autostart"].as_bool())
                    .unwrap_or(false);
                found.push((name, autostart));
            }
        }
    }
    found.sort();
    let mut s = SHARED.state.lock().unwrap();
    s.mods.retain(|m| m.state != ModState::Stopped || found.iter().any(|(n, _)| *n == m.name));
    for (name, autostart) in found {
        match s.mods.iter_mut().find(|m| m.name == name) {
            Some(m) => m.autostart = autostart,
            None => s.mods.push(Mod {
                name,
                state: ModState::Stopped,
                error: None,
                cpu_ms: 0.0,
                memory: 0,
                autostart,
            }),
        }
    }
    s.mods.sort_by(|a, b| a.name.cmp(&b.name));
}

/// The mods and their state.
pub fn mods() -> Vec<Mod> {
    SHARED.state.lock().unwrap().mods.clone()
}

/// Starts mod `name` (again, if it ran before).
pub fn start(name: &str) {
    command(Command::Start(name.to_string()));
}

/// Stops mod `name`: its thread ends, with what it created.
pub fn stop(name: &str) {
    command(Command::Stop(name.to_string()));
}

fn command(c: Command) {
    if SHARED.dir.get().is_none() {
        return;
    }
    SHARED.state.lock().unwrap().commands.push(c);
    SHARED.pending.store(true, std::sync::atomic::Ordering::Release);
}

/// The log lines since the last call.
pub fn take_log() -> Vec<LogLine> {
    SHARED.state.lock().unwrap().log.drain(..).collect()
}

/// Whether a native trapped: the game may now misbehave.
pub fn tainted() -> bool {
    natives::TAINTED.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn log_line(module: &str, level: LogLevel, text: &str) {
    match level {
        LogLevel::Info => log::info!("mod {module}: {text}"),
        LogLevel::Error => log::error!("mod {module}: {text}"),
    }
    let mut s = SHARED.state.lock().unwrap();
    if s.log.len() == LOG_LINES {
        s.log.pop_front();
    }
    s.log.push_back(LogLine { time: SystemTime::now(), module: module.to_string(), level, text: text.to_string() });
}
