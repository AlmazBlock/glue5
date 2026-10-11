//! The engine's own exit: "Exit Game" in the pause menu, or the restart the
//! graphics settings ask for after a change that needs one (texture quality,
//! ...). On PC the game shuts down and its launcher starts it again; here
//! the shutdown never finishes (it waits on work that never comes), so the
//! engine is kept running and the host ends the process instead, starting it
//! again for a restart.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use glue5_game::{Env, orig};

/// What the engine asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Quit,
    /// To start again with the settings it has just saved.
    Restart,
}

static REQUEST: AtomicU8 = AtomicU8::new(0);

/// What the engine has asked for, if it has: the host should end the process.
pub fn requested() -> Option<Exit> {
    match REQUEST.load(Ordering::Relaxed) {
        1 => Some(Exit::Quit),
        2 => Some(Exit::Restart),
        _ => None,
    }
}

/// The engine's "restart pending" flag (a byte, at this address in this
/// build): set by the settings menu once it has saved settings that need a
/// restart (and by the RESTART_GAME script command), read by its exit flow.
const RESTART_PENDING: u64 = 19566058;

/// Hook for `CSystem::WantToExit()`: never lets the main loop end.
pub(crate) unsafe extern "C" fn want_to_exit(env: *mut Env) -> u32 {
    if unsafe { orig::want_to_exit(env) } != 0 {
        request("main loop");
    }
    0
}

/// Hook for `rage::rlShutdown()`, which the engine calls only on its way
/// down (a restart goes straight there, from within a frame): the engine
/// thread stops here, after a moment for the saves it has just started.
pub(crate) unsafe extern "C" fn rl_shutdown(_env: *mut Env) {
    std::thread::sleep(Duration::from_millis(500));
    request("shutdown");
    loop {
        std::thread::park();
    }
}

fn request(from: &str) {
    if requested().is_some() {
        return;
    }
    let restart = crate::rt().memory.mem().u8(RESTART_PENDING) != 0;
    log::info!("the engine asks to {} ({from})", if restart { "restart" } else { "exit" });
    REQUEST.store(if restart { 2 } else { 1 }, Ordering::Relaxed);
}
