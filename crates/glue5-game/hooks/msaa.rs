// Hooks for the runtime's `msaa.rs` (see ../hooks.rs).
&[
    // MSAA stays off: the game's shaders cannot draw it (see msaa.rs). The
    // settings loaded from the file and those the menu asks for; the
    // manager, for the host.
    ("settings_load", "CSettingsManager::Load()"),
    ("settings_request", "CSettingsManager::RequestNewSettings(Settings)"),
    ("settings_instance", "CSettingsManager::GetInstance()"),
]
