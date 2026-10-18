//! Mods: Luau scripts, each in a script thread of the game's own, calling
//! the game's natives.
//!
//! A mod is a folder in the mods folder with a `main.luau` and, optionally,
//! a `mod.json` (`{"autostart": true}`). The application calls [`install`]
//! before the game starts, then [`start`], [`stop`], [`mods`] and [`logs`]
//! from any thread; the mods themselves run on the engine's script update
//! (engine.rs). [`studio`] serves GLUE 5 Studio and the MCP endpoint.

mod checker;
mod engine;
pub mod files;
pub mod natives;
pub mod studio;
mod vm;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModState {
    Stopped,
    Running,
    Error,
}

impl ModState {
    pub fn name(self) -> &'static str {
        match self {
            ModState::Stopped => "stopped",
            ModState::Running => "running",
            ModState::Error => "error",
        }
    }
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
    /// It waits (the engine counts the time).
    pub waiting: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    /// Increasing: what a reader has seen is what is below its cursor.
    pub seq: u64,
    pub time: SystemTime,
    pub module: String,
    pub level: LogLevel,
    pub text: String,
}

/// What `eval` gives back: the values as text, or the error.
pub type EvalResult = Result<String, String>;

enum Command {
    Start(String),
    Stop(String),
    Eval(String, Sender<EvalResult>),
}

struct State {
    mods: Vec<Mod>,
    commands: Vec<Command>,
    log: VecDeque<LogLine>,
    next_seq: u64,
    /// Files written, deleted or renamed, with a sequence number.
    changes: VecDeque<(u64, String)>,
    next_change: u64,
}

struct Shared {
    dir: OnceLock<PathBuf>,
    state: Mutex<State>,
    /// Commands are waiting for the engine.
    pending: AtomicBool,
    /// Changes when a mod's state does (for readers that push it).
    generation: AtomicU64,
}

static SHARED: Shared = Shared {
    dir: OnceLock::new(),
    state: Mutex::new(State {
        mods: Vec::new(),
        commands: Vec::new(),
        log: VecDeque::new(),
        next_seq: 1,
        changes: VecDeque::new(),
        next_change: 1,
    }),
    pending: AtomicBool::new(false),
    generation: AtomicU64::new(0),
};

const LOG_LINES: usize = 2000;

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

/// The mods folder, once installed.
pub fn dir() -> Option<&'static Path> {
    SHARED.dir.get().map(PathBuf::as_path)
}

/// Reads the mods folder again (new or removed mods).
pub fn refresh() {
    let Some(dir) = dir() else { return };
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("main.luau").is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                found.push((name, files::autostart(&path)));
            }
        }
    }
    found.sort();
    let mut s = SHARED.state.lock().unwrap();
    let before = s.mods.len();
    s.mods.retain(|m| m.state != ModState::Stopped || found.iter().any(|(n, _)| *n == m.name));
    let mut changed = before != s.mods.len();
    for (name, autostart) in found {
        match s.mods.iter_mut().find(|m| m.name == name) {
            Some(m) => {
                changed |= m.autostart != autostart;
                m.autostart = autostart;
            }
            None => {
                changed = true;
                s.mods.push(Mod {
                    name,
                    state: ModState::Stopped,
                    error: None,
                    cpu_ms: 0.0,
                    memory: 0,
                    autostart,
                    waiting: false,
                })
            }
        }
    }
    s.mods.sort_by(|a, b| a.name.cmp(&b.name));
    if changed {
        SHARED.generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// The mods and their state.
pub fn mods() -> Vec<Mod> {
    SHARED.state.lock().unwrap().mods.clone()
}

/// Changes whenever [`mods`] would give something else (but the time and
/// memory figures).
pub fn generation() -> u64 {
    SHARED.generation.load(Ordering::Relaxed)
}

/// Starts mod `name` (again, if it ran before).
pub fn start(name: &str) {
    command(Command::Start(name.to_string()));
}

/// Stops mod `name`: its thread ends, with what it created.
pub fn stop(name: &str) {
    command(Command::Stop(name.to_string()));
}

/// Stops mod `name` if it runs, then starts it.
pub fn restart(name: &str) {
    stop(name);
    start(name);
}

/// Runs Luau `code` in the game (in a script thread of its own, as a mod),
/// as an expression if it is one. `reply` gets the values as text.
pub fn eval(code: &str, reply: Sender<EvalResult>) {
    if dir().is_none() {
        let _ = reply.send(Err("mods are off".into()));
        return;
    }
    command(Command::Eval(code.to_string(), reply));
}

fn command(c: Command) {
    if dir().is_none() {
        return;
    }
    SHARED.state.lock().unwrap().commands.push(c);
    SHARED.pending.store(true, Ordering::Release);
}

/// The log lines after `seq` (0: all kept), and the cursor to pass next.
pub fn logs(seq: u64) -> (Vec<LogLine>, u64) {
    let s = SHARED.state.lock().unwrap();
    let lines: Vec<LogLine> = s.log.iter().filter(|l| l.seq > seq).cloned().collect();
    (lines, s.next_seq - 1)
}

/// The files changed after `seq` (by any client), and the cursor to pass next.
pub fn changes(seq: u64) -> (Vec<String>, u64) {
    let s = SHARED.state.lock().unwrap();
    let paths = s.changes.iter().filter(|(n, _)| *n > seq).map(|(_, p)| p.clone()).collect();
    (paths, s.next_change - 1)
}

pub(crate) fn changed(path: &str) {
    let mut s = SHARED.state.lock().unwrap();
    if s.changes.len() == 256 {
        s.changes.pop_front();
    }
    let n = s.next_change;
    s.next_change += 1;
    s.changes.push_back((n, path.to_string()));
}

/// Whether a native trapped: the game may now misbehave.
pub fn tainted() -> bool {
    natives::TAINTED.load(Ordering::Relaxed)
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
    let seq = s.next_seq;
    s.next_seq += 1;
    s.log.push_back(LogLine { seq, time: SystemTime::now(), module: module.to_string(), level, text: text.to_string() });
}
