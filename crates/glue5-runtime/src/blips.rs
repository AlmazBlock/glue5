//! Map blips whose sprite the game data lacks. The scripts are newer than
//! the data and blip later updates' places (businesses, the casino, the
//! car meet...) with sprites the 2014 minimap movie does not have; the
//! minimap then draws a wrong or empty icon. Such a blip is hidden: shown
//! nowhere, whatever display the scripts give it.

use std::collections::HashSet;
use std::sync::Mutex;

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// `CMiniMap`'s sprites, from the minimap movie: `{ i32 id, ConstString
/// name }`, 16 bytes each, sorted by id, which run from 0 without a gap.
const SPRITES: u64 = 19520936;
const SPRITE_COUNT: u64 = 19520944;
/// `eBLIP_PARAMS`.
const BLIP_DISPLAY: u32 = 1;
const BLIP_SPRITE: u32 = 3;
const DISPLAY_NONE: u32 = 0;

/// The blips hidden, and the sprites reported.
static HIDDEN: Mutex<Option<(HashSet<u32>, HashSet<u32>)>> = Mutex::new(None);

/// Hook for `CMiniMap::SetBlipParameter(eBLIP_PARAMS, int, int)`.
pub(crate) unsafe extern "C" fn blip_parameter(env: *mut Env, param: u32, blip: u32, value: u32) {
    match param {
        BLIP_SPRITE => {
            let m = unsafe { Mem::of(env) };
            let count = u32::from(m.u16(SPRITE_COUNT));
            let known = count == 0 || m.u64(SPRITES) == 0 || value < count;
            let mut hidden = HIDDEN.lock().unwrap();
            let (blips, reported) = hidden.get_or_insert_default();
            if known {
                blips.remove(&blip);
            } else {
                if reported.insert(value) {
                    log::info!("blips: sprite {value} is not in the game data, its blips hidden");
                }
                blips.insert(blip);
                drop(hidden);
                unsafe { orig::blip_parameter(env, BLIP_DISPLAY, blip, DISPLAY_NONE) };
                return;
            }
        }
        BLIP_DISPLAY if value != DISPLAY_NONE => {
            let hidden = HIDDEN.lock().unwrap();
            if hidden.as_ref().is_some_and(|(blips, _)| blips.contains(&blip)) {
                return;
            }
        }
        _ => {}
    }
    unsafe { orig::blip_parameter(env, param, blip, value) }
}

pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.blip_parameter = Some(blip_parameter);
}
