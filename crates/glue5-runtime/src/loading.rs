//! Loading at full speed.
//!
//! The engine's frame limiter, at the end of `CSystem::EndRender` on the
//! render thread, waits until each frame has had its share of a second: the
//! game's VSync setting in 60 Hz intervals, which `-frameLimit` sets (1: 60
//! fps, 2: 30). It waits behind the loading screens and black screens as
//! well, and the loading behind them goes at the pace of the frames: from
//! the start of story mode to the first frame of the world, 16 s at 60 fps
//! and 22 s at 30 (macOS, headless), more on a phone.
//!
//! While the loading screens are up or the screen is faded out to black,
//! `EndRender` runs with the setting at 0 (no limit), put back as soon as it
//! returns; the host's own frame pacing follows [`active`]. The setting is
//! compared with others only while new settings from the menu wait to be
//! applied, which a loading screen or a black screen does not show.

use std::sync::atomic::{AtomicBool, Ordering};

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// `Settings::m_video.m_VSync`: frames last this many 60 Hz intervals at
/// least (0: no limit).
const VSYNC: u64 = 256;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The game is loading behind a loading screen or a black screen: frames
/// should not wait.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Hook for `CSystem::EndRender()`.
pub(crate) unsafe extern "C" fn end_render(env: *mut Env) {
    let loading = unsafe { orig::loading_screens_active(env) != 0 || orig::faded_out(env) != 0 };
    if loading != ACTIVE.swap(loading, Ordering::Relaxed) {
        log::debug!("frame limit {}", if loading { "lifted (loading)" } else { "back" });
    }
    if !loading {
        return unsafe { orig::end_render(env) };
    }
    let mem = unsafe { Mem::of(env) };
    let vsync = unsafe { orig::get_settings(env, orig::settings_instance(env)) } + VSYNC;
    let limit = mem.u32(vsync);
    mem.set_u32(vsync, 0);
    unsafe { orig::end_render(env) };
    // Unless the settings were applied meanwhile.
    if mem.u32(vsync) == 0 {
        mem.set_u32(vsync, limit);
    }
}
