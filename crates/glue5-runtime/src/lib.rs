//! Host runtime for the recompiled game: what the browser build got from
//! Emscripten's JS glue, game.js, the workers and the page.
//!
//! [`start`] reserves the wasm memory, installs the imports and runs the
//! engine's `main` on its own thread. Engine threads (pthreads) become native
//! threads, each with its own instance over the shared memory.

pub mod audio;
mod device_reset;
pub mod exit;
pub mod game_settings;
mod gamelog;
pub mod gpu;
mod imports;
pub mod input;
pub mod memory;
mod msaa;
pub mod qos;
mod readback;
mod scenario;
mod skin;
mod threads;
mod time;
pub mod tour;
mod trap_site;
pub mod vfs;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use anyhow::{Context, Result};

pub use glue5_game::GAME_BUILD;

/// The CPU cores the process may use, asked once: on Linux and Android the
/// standard library reads the cgroup quotas each time, which Android's
/// sandbox denies (and logs).
pub fn cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
}

/// The game directory's layout, as the whale5 server serves it: the engine's
/// files under `data/`, this build's translated shaders under
/// `b/<build>/shaders/`.
pub mod layout {
    use std::path::{Path, PathBuf};

    pub fn data_dir(game: &Path) -> PathBuf {
        game.join("data")
    }

    pub fn shader_dir(game: &Path) -> PathBuf {
        game.join("b").join(super::GAME_BUILD).join("shaders")
    }

    /// Whether `game` holds the game, for this build.
    pub fn is_game_dir(game: &Path) -> bool {
        data_dir(game).is_dir() && shader_dir(game).join("index.json").is_file()
    }
}

use crate::memory::{Mem, WasmMemory};
use crate::vfs::Vfs;

/// How the game is started.
#[derive(Clone, Debug)]
pub struct Config {
    /// The game directory: `b/<build>/` (shaders) and `data/`.
    pub game_dir: PathBuf,
    /// Where saves and settings go.
    pub user_dir: PathBuf,
    /// Back buffer size.
    pub width: u32,
    pub height: u32,
    /// 1 = story, 2 = sandbox (GTA V map), 3 = sandbox (env_test).
    pub start_mode: u32,
    /// Smaller engine pools (with [`game_settings::apply_low_preset`], the
    /// browser build's `?low=1`).
    pub low_memory: bool,
    /// Frames per second at most: 0 lets the engine run as fast as the
    /// renderer presents (the display's refresh with vsync); 60 or 30 use
    /// the engine's own limiter.
    pub max_fps: u32,
    /// The game's benchmark (its fly-through of the map) instead of the
    /// game; the engine writes the results in the user directory
    /// (`Documents/Rockstar Games/GTA V/Benchmarks`) and asks to quit.
    pub benchmark: bool,
}

impl Config {
    /// The engine's command line, as the browser page builds it.
    pub fn engine_args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "-rootdir=/game/",
            "-wgpu",
            "-nodisplaycalibration",
            "-noSocialClub",
            "-pc:nosocialclub",
            "-nonetwork",
            "-output",
        ]
        .map(String::from)
        .into();
        args.push(format!("-width={}", self.width));
        args.push(format!("-height={}", self.height));
        args.push("-forceResolution".into());
        args.extend(["-Script_all=warning", "-replay_all=warning", "-fastscriptcore"].map(String::from));
        // This build is a BankRelease one: its debug pad took the first pad to
        // send input, and START and Back with it (START never opened the
        // pause menu, and after it the pad's other buttons went to the debug
        // tools). Pads as in a retail build.
        args.extend(["-disableDebugPadButtons", "-disallowdebugpad"].map(String::from));
        // The engine's limiter counts 60 Hz intervals per frame (0: off).
        let frame_limit = match self.max_fps {
            0 => 0,
            fps => (60 / fps.clamp(1, 60)).max(1),
        };
        args.push(format!("-frameLimit={frame_limit}"));
        args.push(format!("-wasmstartchoice={}", self.start_mode));
        if self.benchmark {
            args.extend(["-benchmark", "-benchmarkIterations=1"].map(String::from));
        }
        // No quality switches: they would override the game's own settings
        // (its menu) at every start.
        // More engine switches, for experiments.
        if let Ok(extra) = std::env::var("GLUE5_ENGINE_ARGS") {
            args.extend(extra.split_whitespace().map(String::from));
        }
        args
    }
}

/// Process-wide state shared by imports and host threads.
pub(crate) struct Runtime {
    pub config: Config,
    pub memory: WasmMemory,
    pub vfs: Vfs,
    pub clock: time::Clock,
    /// Addresses the engine publishes for the host (0 until published).
    pub input_block: AtomicU64,
    pub audio_ring: AtomicU64,
    pub gpu_ring: AtomicU64,
    /// Taken when the engine starts its GPU worker.
    pub gpu_backend: Mutex<Option<gpu::GpuBackend>>,
    pub threads: threads::Registry,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub(crate) fn rt() -> &'static Runtime {
    RUNTIME.get().expect("runtime not started")
}

/// A running game.
pub struct Game {
    engine: JoinHandle<i32>,
}

impl Game {
    /// Waits for the engine's `main` to return (it normally never does).
    pub fn join(self) -> i32 {
        self.engine.join().unwrap_or(1)
    }

    /// The wasm memory, for host subsystems (input, audio, GPU).
    pub fn mem(&self) -> Mem {
        rt().memory.mem()
    }

    /// The engine's audio output (silent until the engine has started its mixer).
    pub fn audio(&self) -> audio::AudioOut {
        audio::AudioOut
    }

    /// The engine's keyboard/mouse input block, once it has published it.
    pub fn input(&self) -> Option<input::InputBlock> {
        nonzero(rt().input_block.load(Ordering::Acquire)).map(|at| input::InputBlock::new(rt().memory.mem(), at))
    }
}

fn nonzero(v: u64) -> Option<u64> {
    (v != 0).then_some(v)
}

/// Starts the game, rendering with `gpu` (e.g. [`gpu::null`]). Only one game
/// can run per process.
pub fn start(config: Config, gpu: gpu::GpuBackend) -> Result<Game> {
    let data = layout::data_dir(&config.game_dir);
    std::fs::create_dir_all(&config.user_dir)?;
    let log = gamelog::start(&config.user_dir).context("creating the engine log")?;
    log::info!("engine log: {} (GLUE5_ENGINE_LOG=stdout also prints it)", log.display());
    glue5_game::set_trap_hook(trap_site::record);
    time::fine_timers();
    let vfs = Vfs::new(data.clone(), config.user_dir.clone()).with_context(|| format!("opening {}", data.display()))?;
    let memory = WasmMemory::reserve()?;
    let runtime = Runtime {
        config,
        memory,
        vfs,
        clock: time::Clock::new(),
        input_block: AtomicU64::new(0),
        audio_ring: AtomicU64::new(0),
        gpu_ring: AtomicU64::new(0),
        gpu_backend: Mutex::new(Some(gpu)),
        threads: threads::Registry::default(),
    };
    if RUNTIME.set(runtime).is_err() {
        anyhow::bail!("the game is already running");
    }
    let hooks = glue5_game::Hooks {
        pad_update: Some(input::pad_update),
        text_box_open: Some(input::text_box_open_hook),
        text_box_close: Some(input::text_box_close_hook),
        want_to_exit: Some(exit::want_to_exit),
        rl_shutdown: Some(exit::rl_shutdown),
        grow_heap: Some(memory::grow_heap),
        heap_size: None,
        memory_available: Some(memory::memory_available),
        largest_block: Some(memory::largest_block),
        skin_light_pass: Some(skin::skin_light_pass),
        get_settings: Some(skin::get_settings),
        settings_load: Some(msaa::settings_load),
        settings_request: Some(msaa::settings_request),
        settings_instance: None,
        device_lost: Some(device_reset::device_lost),
        gpu_fence: None,
        sprite_depth_texture: None,
        wait_readback: None,
        buffer_dtor: Some(readback::buffer_dtor),
        buffer_delete: Some(readback::buffer_delete),
        texture1d_dtor: Some(readback::texture1d_dtor),
        texture1d_delete: Some(readback::texture1d_delete),
        texture2d_dtor: Some(readback::texture2d_dtor),
        texture2d_delete: Some(readback::texture2d_delete),
        texture3d_dtor: Some(readback::texture3d_dtor),
        texture3d_delete: Some(readback::texture3d_delete),
        is_position_clear_for_ped: Some(scenario::is_position_clear_for_ped),
        warp_update: Some(tour::warp_update),
        find_player_ped: None,
        warp: None,
        clock_set_time: None,
        weather_index: None,
        weather_force_now: None,
        height_max: None,
    };
    glue5_game::install(glue5_game::imports_from!(imports), hooks);
    let engine = threads::spawn_main()?;
    Ok(Game { engine })
}
