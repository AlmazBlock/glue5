//! MSAA stays off.
//!
//! The game's shaders are its DirectX 10 set (`win32_40`), whose multisampled
//! effects are an older revision than the code that drives them: the code
//! asks `imMS`'s depth resolve for a fourth pass it does not have, indexes
//! `postfxMS`'s technique as `postfx`'s (ten passes longer), and dispatches a
//! multisampled depth reduction of the tiled lighting that has no
//! translation. The game's D3D11 layer also makes x2 targets single-sampled
//! (WebGPU has no x2). With MSAA on, `grcEffect::BeginPass` traps on the
//! missing pass and the render thread stops at the first frame of a scene;
//! past that, post-processing would draw the wrong passes into a black image.
//!
//! The engine turns MSAA off itself on the DirectX 10 feature set (`-DX10`).
//! Here the settings it loads and those its menu asks for have it off; it
//! saves them that way. The game's files stay as installed.

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// `Settings::m_graphics.m_MSAA`: 0 off, else the sample count.
const MSAA: u64 = 60;

fn turn_off(env: *mut Env, settings: u64) {
    let mem = unsafe { Mem::of(env) };
    let samples = mem.u32(settings + MSAA);
    if samples != 0 {
        log::info!("game settings: MSAA x{samples} off (the game's shaders cannot draw it)");
        mem.set_u32(settings + MSAA, 0);
    }
}

/// Hook for `CSettingsManager::Load()`: the settings file's.
pub(crate) unsafe extern "C" fn settings_load(env: *mut Env) {
    unsafe { orig::settings_load(env) };
    let manager = unsafe { orig::settings_instance(env) };
    turn_off(env, unsafe { orig::get_settings(env, manager) });
}

/// Hook for `CSettingsManager::RequestNewSettings(Settings)`: the menu's.
pub(crate) unsafe extern "C" fn settings_request(env: *mut Env, settings: u64) {
    turn_off(env, settings);
    unsafe { orig::settings_request(env, settings) }
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.settings_load = Some(settings_load);
    hooks.settings_request = Some(settings_request);
}
