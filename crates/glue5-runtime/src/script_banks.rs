//! Sound banks the scripts ask for that this game data lacks.
//!
//! The scripts are a later version than the game data, and some of the
//! banks they load are not in it (GTA Online's `DLC_GTAO\SNACKS`, which
//! `MP_WASTED` and `HUD_321_GO` are mapped to as well): the bank's file is
//! not found and its wave slot stays failed. A script's request
//! (`audScriptAudioEntity::RequestScriptBank`) then answers "not yet"
//! forever, and scripts waiting on it wait forever. Freemode's interaction
//! animations (the quick action, pointing...) wait for the snacks bank with
//! sprint, attacks, the weapon wheel and the phone disabled until the
//! player dies. Here a failed bank counts as loaded for a script's
//! request: the script goes on, without those sounds.

use std::cell::Cell;

use glue5_game::{Env, orig};

/// `audWaveSlot`'s loading status: loaded, loading, failed.
const LOADED: u32 = 0;
const FAILED: u32 = 2;

thread_local! {
    /// Inside a script's bank request.
    static REQUEST: Cell<bool> = const { Cell::new(false) };
}

/// Hook for `audScriptAudioEntity::RequestScriptBank(unsigned int, bool,
/// unsigned int)`.
pub(crate) unsafe extern "C" fn request(env: *mut Env, bank: u32, over_network: u32, players: u32) -> u32 {
    REQUEST.with(|r| r.set(true));
    let loaded = unsafe { orig::script_bank_request(env, bank, over_network, players) };
    REQUEST.with(|r| r.set(false));
    loaded
}

/// Hook for `rage::audWaveSlot::GetBankLoadingStatus(unsigned int) const`.
pub(crate) unsafe extern "C" fn loading_status(env: *mut Env, slot: u64, bank: u32) -> u32 {
    let status = unsafe { orig::bank_loading_status(env, slot, bank) };
    if status == FAILED && REQUEST.with(Cell::get) { LOADED } else { status }
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.script_bank_request = Some(request);
    hooks.bank_loading_status = Some(loading_status);
}
