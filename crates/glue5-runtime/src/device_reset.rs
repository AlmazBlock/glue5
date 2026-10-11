//! A device reset (the window resized): what it frees while still in use.
//!
//! `grcDevice::DeviceLost` (render thread, between frames) destroys the
//! screen-sized render targets, with them the staging copies some keep for
//! the CPU, and releases the resource cache; new targets are made later, on
//! the main thread. Two things still point at what it frees:
//!
//! - A readback in flight. The game's D3D layer frees a staging texture's
//!   memory before waiting for the readback into it (the replay thumbnail's
//!   was seen in flight), and the GPU worker then writes the pixels
//!   into freed heap: a later `malloc` or `free` crashes. The engine's own
//!   GPU fence first lets every readback asked so far land.
//! - The 2D sprite shader's depth texture. `CVisualEffects::RenderCoronas`
//!   gives it the scene's depth target (`CSprite2d::SetDepthTexture`) only
//!   when it draws coronas a certain way, so the shader can hold it for a
//!   long time; every technique of that shader binds it, the minimap's
//!   among them, which then asks the destroyed target for its view. The
//!   shader is given no depth texture instead (an empty one is not bound)
//!   until the coronas give it the new one.

use glue5_game::{Env, orig};

/// Hook for `rage::grcDevice::DeviceLost()`.
pub(crate) unsafe extern "C" fn device_lost(env: *mut Env) {
    unsafe {
        orig::gpu_fence(env);
        orig::device_lost(env);
        orig::sprite_depth_texture(env, 0);
    }
}
