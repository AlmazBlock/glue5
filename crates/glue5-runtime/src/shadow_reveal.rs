//! The cascade shadows' cloud shadows.
//!
//! The screen-space shadow pass (`ShadowRevealSetup`, the
//! `cascadeshadows_rendering` effect) darkens the sunlight where clouds pass:
//! it reads a cloud texture at the pixel's place in the world and maps the
//! value through a small table the engine renders each frame
//! (`CCSMGlobals`' smooth step). The code sets both as globals,
//! `gCSMCloudTexture` and `gCSMSmoothStepTexture`; the game's effect, an
//! older revision than the code, has them as its own locals, `CloudTexture`
//! and `SmoothStepTexture`. Unset, the pass read whatever their slots held
//! last: another cloud-sized texture, and the tiled lighting's depth bounds
//! of the screen, so the sunlight on everything near the camera went on and
//! off with what crossed the middle of the screen (grass in front of the
//! camera, for one).
//!
//! What the code sets the globals to goes into the locals too, before each
//! shadow pass. With effects that have the globals, nothing changes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use glue5_game::{Env, exports, orig};

use crate::memory::Mem;

/// The code's global and the effect's local, for each texture.
const VARS: [(&str, &str); 2] = [("gCSMCloudTexture", "CloudTexture"), ("gCSMSmoothStepTexture", "SmoothStepTexture")];

/// The globals' handles (0: not looked up yet, or unknown to the effects).
static HANDLES: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
/// The textures the code last set the globals to.
static TEXTURES: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
/// Per shader of the pass: its locals' handles (0: it has no such local).
static LOCALS: Mutex<Vec<(u64, [u32; 2])>> = Mutex::new(Vec::new());

/// Hook for `rage::grcEffect::LookupGlobalVar(char const*, bool)` (the
/// flag is gone from the build: its callers all pass false).
pub(crate) unsafe extern "C" fn lookup_global_var(env: *mut Env, name: u64) -> u32 {
    let handle = unsafe { orig::lookup_global_var(env, name) };
    if handle != 0 {
        let name = unsafe { Mem::of(env) }.str(name);
        if let Some(i) = VARS.iter().position(|(global, _)| *global == name) {
            HANDLES[i].store(handle, Ordering::Relaxed);
        }
    }
    handle
}

/// Hook for `rage::grcEffect::SetGlobalVar(grcEffectGlobalVar, grcTexture const*)`.
pub(crate) unsafe extern "C" fn set_global_texture(env: *mut Env, var: u32, texture: u64) {
    if var != 0
        && let Some(i) = HANDLES.iter().position(|h| h.load(Ordering::Relaxed) == var)
    {
        TEXTURES[i].store(texture, Ordering::Relaxed);
    }
    unsafe { orig::set_global_texture(env, var, texture) }
}

/// Hook for `ShadowRevealSetup::SetVars(...) const`: sets the pass's
/// variables, then the globals' textures into the locals.
pub(crate) unsafe extern "C" fn shadow_reveal_set_vars(env: *mut Env, setup: u64, p1: u64, p2: u64, p3: u64) {
    unsafe { orig::shadow_reveal_set_vars(env, setup, p1, p2, p3) };
    // `ShadowRevealSetup` starts with its grmShader, which starts with its
    // instance data; the shader's effect follows at +8.
    let shader = unsafe { Mem::of(env) }.u64(setup);
    if shader == 0 {
        return;
    }
    let locals = unsafe { locals(env, shader) };
    for (i, &local) in locals.iter().enumerate() {
        let texture = TEXTURES[i].load(Ordering::Relaxed);
        if local != 0 && texture != 0 {
            unsafe { orig::effect_set_texture(env, shader, local, texture) };
        }
    }
}

/// The shader's handles of the locals (looked up once per shader).
unsafe fn locals(env: *mut Env, shader: u64) -> [u32; 2] {
    let mut known = LOCALS.lock().unwrap();
    if let Some(&(_, vars)) = known.iter().find(|(s, _)| *s == shader) {
        return vars;
    }
    let effect = unsafe { Mem::of(env) }.u64(shader + 8);
    let vars = VARS.map(|(_, local)| if effect == 0 { 0 } else { unsafe { lookup_var(env, effect, local) } });
    for ((global, local), var) in VARS.iter().zip(vars) {
        if var != 0 {
            log::info!("shadows: the shadow pass's {local} takes the code's {global}");
        }
    }
    known.push((shader, vars));
    vars
}

/// `grcEffect::LookupVar(name, false)`, the name on the wasm stack.
unsafe fn lookup_var(env: *mut Env, effect: u64, name: &str) -> u32 {
    let Ok(top) = (unsafe { exports::emscripten_stack_get_current(env) }) else { return 0 };
    let Ok(at) = (unsafe { exports::_emscripten_stack_alloc(env, name.len() as u64 + 1) }) else { return 0 };
    unsafe { Mem::of(env) }.write_str(at, name, name.len() + 1);
    let var = unsafe { orig::effect_lookup_var(env, effect, at, 0) };
    let _ = unsafe { exports::_emscripten_stack_restore(env, top) };
    var
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.lookup_global_var = Some(lookup_global_var);
    hooks.set_global_texture = Some(set_global_texture);
    hooks.shadow_reveal_set_vars = Some(shadow_reveal_set_vars);
}
