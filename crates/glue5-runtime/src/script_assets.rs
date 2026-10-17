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
//! - Models and text blocks of later updates (golf waits for two models,
//!   the awards' texts `MPAWD` and the club emblems' textures).
//!
//! Each counts as loaded for the scripts here: they go on without those
//! sounds, textures, models or texts.
//!
//! Whole places are missing too: GTA Online's "simple interiors" (the
//! casino, bunkers, hangars, nightclubs, the arcade...), DLC from 2016 on.
//! Freemode starts `AM_MP_SMPL_INTERIOR_EXT` near each one's entrance, and
//! walking in takes the player's control for the entry scene, which never
//! ends: the casino's waits for its door (a model this data lacks) to be
//! created, and the interior behind it does not exist either. That script
//! never loads here, so the entrances do nothing.
//!
//! And screens: the phone's movie (`cellphone_ifruit`) has no app for GTA
//! Online's boss agency (`CELL_BOSSAGE`), which the scripts put on the
//! phone. Opening it, the movie's `LOAD_APP` and `CELLPHONE_APP` call each
//! other without end; each ActionScript call pushes a diagnostic context,
//! and at the 33rd the engine stops ("MessageCommon - too many contexts
//! pushed"). ActionScript calls nest at most `AS_DEPTH` deep here: deeper
//! ones return at once, and the app shows nothing instead.

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use glue5_game::{Env, exports, orig};

use crate::memory::Mem;

/// Scripts for places this game data lacks (see above).
const MISSING_PLACES: &[&str] = &["AM_MP_SMPL_INTERIOR_EXT"];

fn for_missing_place(m: Mem, name: u64) -> bool {
    let name = m.str(name);
    MISSING_PLACES.iter().any(|s| s.eq_ignore_ascii_case(&name))
}

/// Hook for `script_commands::RequestScript(char const*)`: REQUEST_SCRIPT.
pub(crate) unsafe extern "C" fn script_request(env: *mut Env, name: u64) {
    if !for_missing_place(unsafe { Mem::of(env) }, name) {
        unsafe { orig::script_request(env, name) };
    }
}

/// Hook for `script_commands::HasScriptLoaded(char const*)`:
/// HAS_SCRIPT_LOADED, never for those scripts (freemode asks again while
/// the player is near an entrance).
pub(crate) unsafe extern "C" fn script_loaded(env: *mut Env, name: u64) -> u32 {
    if for_missing_place(unsafe { Mem::of(env) }, name) { 0 } else { unsafe { orig::script_loaded(env, name) } }
}

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

/// Hook for `streaming_commands::HasModelLoaded(int)`: HAS_MODEL_LOADED,
/// yes for a model the data lacks (no archetype has its hash).
pub(crate) unsafe extern "C" fn model_loaded(env: *mut Env, hash: u32) -> u32 {
    if unsafe { orig::script_model_loaded(env, hash) } != 0 {
        return 1;
    }
    let Ok(top) = (unsafe { exports::emscripten_stack_get_current(env) }) else { return 0 };
    let Ok(id) = (unsafe { exports::_emscripten_stack_alloc(env, 16) }) else { return 0 };
    let archetype = unsafe { orig::archetype_from_hash(env, hash, id) };
    let _ = unsafe { exports::_emscripten_stack_restore(env, top) };
    (archetype == 0) as u32
}

/// The additional text slots (CTextFile's 23) and the block each was last
/// asked to load, until the slot is freed.
const TEXT_SLOTS: usize = 23;
static ASKED: Mutex<[Option<String>; TEXT_SLOTS]> = Mutex::new([const { None }; TEXT_SLOTS]);

/// Whether a block that is not loaded counts as loaded: only one asked for
/// in that slot whose load is over (the data lacks it). One never asked is
/// not: the engine asks whether a block is loaded before it asks for it
/// (a cutscene's subtitles, a movie's), and so do scripts.
fn failed(requesting: bool, asked: Option<&str>, name: &str) -> bool {
    !requesting && asked.is_some_and(|a| a.eq_ignore_ascii_case(name))
}

/// Hook for `CTextFile::RequestAdditionalText(char const*, int,
/// CTextFile::eLocationOfTextBlock)`: the block asked for, kept for its slot.
pub(crate) unsafe extern "C" fn text_request(env: *mut Env, name: u64, slot: u32, location: u32) {
    // First the request, which frees the slot of what it held.
    unsafe { orig::text_request(env, name, slot, location) };
    if let Some(asked) = ASKED.lock().unwrap().get_mut(slot as usize) {
        *asked = Some(unsafe { Mem::of(env) }.str(name));
    }
}

/// Hook for `CTextFile::FreeTextSlot(int, bool)`: the slot asks for nothing.
pub(crate) unsafe extern "C" fn text_free(env: *mut Env, slot: u32) {
    if let Some(asked) = ASKED.lock().unwrap().get_mut(slot as usize) {
        *asked = None;
    }
    unsafe { orig::text_free(env, slot) };
}

/// Hook for `CTextFile::HasThisAdditionalTextLoaded(char const*, int)`:
/// yes too for a block asked for whose load ended without it (see `failed`).
pub(crate) unsafe extern "C" fn additional_text_loaded(env: *mut Env, name: u64, slot: u32) -> u32 {
    if unsafe { orig::additional_text_loaded(env, name, slot) } != 0 {
        return 1;
    }
    let requesting = unsafe { orig::additional_text_requesting(env, name, slot) } != 0;
    let asked = ASKED.lock().unwrap().get(slot as usize).cloned().flatten();
    failed(requesting, asked.as_deref(), &unsafe { Mem::of(env) }.str(name)) as u32
}

/// How deep ActionScript calls nest on a thread: the engine keeps 32
/// diagnostic contexts per thread, one per call and a few of its own; no
/// movie of the game's goes near.
const AS_DEPTH: u32 = 24;

thread_local! {
    /// ActionScript calls under way on this thread.
    static AS_NESTED: Cell<u32> = const { Cell::new(0) };
}

/// An ActionScript call, unless `AS_DEPTH` are already under way (see
/// above): then it returns at once, its result undefined. True if it ran.
fn nested(call: impl FnOnce()) -> bool {
    let depth = AS_NESTED.get();
    if depth >= AS_DEPTH {
        return false;
    }
    AS_NESTED.set(depth + 1);
    call();
    AS_NESTED.set(depth);
    true
}

static AS_STOPPED: AtomicBool = AtomicBool::new(false);

fn stopped(env: *mut Env, name: u64) {
    if !AS_STOPPED.swap(true, Ordering::Relaxed) {
        let name = if name == 0 { String::new() } else { unsafe { Mem::of(env) }.str(name) };
        log::warn!("scaleform: ActionScript calls nested {AS_DEPTH} deep, stopped at {name} (a movie older than the scripts)");
    }
}

/// Hook for `GASAsFunctionObject::Invoke(GASFnCall const&, GASLocalFrame*,
/// char const*)`: a function of a movie's ActionScript.
pub(crate) unsafe extern "C" fn as_function_invoke(env: *mut Env, this: u64, call: u64, frame: u64, name: u64) {
    if !nested(|| unsafe { orig::as_function_invoke(env, this, call, frame, name) }) {
        stopped(env, name);
    }
}

/// Hook for `GASUserDefinedFunctionObject::Invoke(...)`: likewise.
pub(crate) unsafe extern "C" fn user_function_invoke(env: *mut Env, this: u64, call: u64, frame: u64, name: u64) {
    if !nested(|| unsafe { orig::user_function_invoke(env, this, call, frame, name) }) {
        stopped(env, name);
    }
}

/// This module's hooks.
pub(crate) fn hooks(hooks: &mut glue5_game::Hooks) {
    hooks.as_function_invoke = Some(as_function_invoke);
    hooks.user_function_invoke = Some(user_function_invoke);
    hooks.script_model_loaded = Some(model_loaded);
    hooks.additional_text_loaded = Some(additional_text_loaded);
    hooks.text_request = Some(text_request);
    hooks.text_free = Some(text_free);
    hooks.script_bank_request = Some(bank_request);
    hooks.bank_loading_status = Some(bank_loading_status);
    hooks.script_txd_loaded = Some(txd_loaded);
    hooks.script_request = Some(script_request);
    hooks.script_loaded = Some(script_loaded);
}

#[cfg(test)]
mod tests {
    use super::{AS_DEPTH, AS_NESTED, failed, nested};

    /// ActionScript that calls itself without end (the phone's LOAD_APP and
    /// CELLPHONE_APP for an app its movie lacks) stops at AS_DEPTH instead of
    /// running the engine out of diagnostic contexts.
    #[test]
    fn actionscript_calling_itself_stops() {
        fn call(calls: &mut u32) {
            *calls += 1;
            nested(|| call(calls));
        }
        let mut calls = 0;
        assert!(nested(|| call(&mut calls)));
        assert_eq!(calls, AS_DEPTH, "each nested call ran once, up to the limit");
        assert_eq!(AS_NESTED.get(), 0, "unwound");
        // Calls in turn (not nested) are not limited.
        for _ in 0..100 {
            assert!(nested(|| ()));
        }
    }

    #[test]
    fn only_a_block_asked_for_counts_as_loaded_when_its_load_failed() {
        // Never asked: the engine and scripts ask before they request it.
        assert!(!failed(false, None, "MPAWD"));
        // Another block asked in that slot.
        assert!(!failed(false, Some("FMMC"), "MPAWD"));
        // Asked, still loading.
        assert!(!failed(true, Some("MPAWD"), "MPAWD"));
        // Asked, and its load ended without it.
        assert!(failed(false, Some("mpawd"), "MPAWD"));
    }
}
