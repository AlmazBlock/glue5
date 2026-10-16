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
    // REQUEST_SCRIPT and HAS_SCRIPT_LOADED: scripts for places the game
    // data lacks are never loaded.
    ("script_request", "script_commands::RequestScript(char const*)"),
    ("script_loaded", "script_commands::HasScriptLoaded(char const*)"),
    // HAS_MODEL_LOADED: a model this game data lacks counts as loaded (its
    // archetype is looked up by hash).
    ("script_model_loaded", "streaming_commands::HasModelLoaded(int)"),
    ("archetype_from_hash", "rage::fwArchetypeManager::GetArchetypeFromHashKey(unsigned int, unsigned int*)"),
    // HAS_THIS_ADDITIONAL_TEXT_LOADED: a text block whose load ended
    // without it (this data lacks it) counts as loaded.
    ("additional_text_loaded", "CTextFile::HasThisAdditionalTextLoaded(char const*, int)"),
    ("additional_text_requesting", "CTextFile::IsRequestingThisAdditionalText(char const*, int)"),
]
