//! Assets the scripts wait for that this game data lacks.
//!
//! The scripts are a later version than the game data, and some of what
//! they load is not in it. The engine never finishes loading it, and a
//! script that waits for it waits forever:
//!
//! - Sound banks (GTA Online's `DLC_GTAO\SNACKS`, which `MP_WASTED` and
//!   `HUD_321_GO` are mapped to as well): the bank's file is not found and
//!   its wave slot stays failed, so a script's request
//!   (`audScriptAudioEntity::RequestScriptBank`) answers "not yet". Freemode's
//!   interaction animations (the quick action, pointing...) wait for the
//!   snacks bank with sprint, attacks, the weapon wheel and the phone
//!   disabled until the player dies.
//! - Texture dictionaries (GTA Online's `MPShopSale`): one the streaming
//!   store has no slot for is never loaded, so the interaction menu, which
//!   waits for it with its other assets, opens without ever being drawn.
//!
//! Either counts as loaded for the scripts here: they go on without those
//! sounds or textures.

use std::cell::Cell;

use glue5_game::{Env, orig};

use crate::memory::Mem;

/// `audWaveSlot`'s loading status: loaded, loading, failed.
const LOADED: u32 = 0;
const FAILED: u32 = 2;

thread_local! {
    /// Inside a script's bank request.
    static REQUEST: Cell<bool> = const { Cell::new(false) };
}

/// Hook for `audScriptAudioEntity::RequestScriptBank(unsigned int, bool,
/// unsigned int)`.
pub(crate) unsafe extern "C" fn bank_request(env: *mut Env, bank: u32, over_network: u32, players: u32) -> u32 {
    REQUEST.with(|r| r.set(true));
    let loaded = unsafe { orig::script_bank_request(env, bank, over_network, players) };
    REQUEST.with(|r| r.set(false));
    loaded
}

/// Hook for `rage::audWaveSlot::GetBankLoadingStatus(unsigned int) const`.
pub(crate) unsafe extern "C" fn bank_loading_status(env: *mut Env, slot: u64, bank: u32) -> u32 {
    let status = unsafe { orig::bank_loading_status(env, slot, bank) };
    if status == FAILED && REQUEST.with(Cell::get) { LOADED } else { status }
}

/// The texture dictionary store's map from name hashes to slots, in this
/// build: the bucket count, the buckets (the first entry's index each), the
/// entries ({ u32 hash, u32 slot, u32 next }).
const TXD_BUCKET_COUNT: u64 = 11097520;
const TXD_BUCKETS: u64 = 11097512;
const TXD_ENTRIES: u64 = 11097504;
const NONE: u32 = u32::MAX;

/// Whether the texture dictionary store has a slot for this name hash.
fn txd_slot_exists(m: Mem, hash: u32) -> bool {
    let buckets = m.u32(TXD_BUCKET_COUNT);
    if buckets == 0 {
        return false;
    }
    let entries = m.u64(TXD_ENTRIES);
    let mut i = m.u32(m.u64(TXD_BUCKETS) + u64::from(hash % buckets) * 4);
    while i != NONE {
        let entry = entries + u64::from(i) * 12;
        if m.u32(entry) == hash {
            return m.u32(entry + 4) != NONE;
        }
        i = m.u32(entry + 8);
    }
    false
}

/// Hook for `graphics_commands::HasStreamedTxdLoaded(char const*)`.
pub(crate) unsafe extern "C" fn txd_loaded(env: *mut Env, name: u64) -> u32 {
    if unsafe { orig::script_txd_loaded(env, name) } != 0 {
        return 1;
    }
    let hash = unsafe { orig::hash_string(env, 0, name) };
    !txd_slot_exists(unsafe { Mem::of(env) }, hash) as u32
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.script_bank_request = Some(bank_request);
    hooks.bank_loading_status = Some(bank_loading_status);
    hooks.script_txd_loaded = Some(txd_loaded);
}
