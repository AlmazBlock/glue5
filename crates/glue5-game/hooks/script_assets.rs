// Hooks for the runtime's `script_assets.rs` (see ../hooks.rs).
&[
    // A script's request for a sound bank, and the bank's loading status
    // as the request reads it: a bank this game data lacks counts as loaded.
    ("script_bank_request", "audScriptAudioEntity::RequestScriptBank(unsigned int, bool, unsigned int)"),
    ("bank_loading_status", "rage::audWaveSlot::GetBankLoadingStatus(unsigned int) const"),
    // HAS_STREAMED_TEXTURE_DICT_LOADED: a dictionary this game data lacks
    // counts as loaded. The hash names the dictionary in the store.
    ("script_txd_loaded", "graphics_commands::HasStreamedTxdLoaded(char const*)"),
    ("hash_string", "rage::atHashStringNamespaceSupport::ComputeHash(rage::atHashStringNamespaces, char const*)"),
]
