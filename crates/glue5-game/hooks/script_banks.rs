// Hooks for the runtime's `script_banks.rs` (see ../hooks.rs).
&[
    // A script's request for a sound bank, and the bank's loading status
    // as the request reads it: a bank this game data lacks counts as loaded.
    ("script_bank_request", "audScriptAudioEntity::RequestScriptBank(unsigned int, bool, unsigned int)"),
    ("bank_loading_status", "rage::audWaveSlot::GetBankLoadingStatus(unsigned int) const"),
]
