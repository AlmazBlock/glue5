// Hooks for the runtime's `online.rs` (see ../hooks.rs).
&[
    // A signed-in local gamer: Social Club's stand-ins go in at the live
    // manager's start; its update keeps the network up (vnet.rs).
    ("rl_gamer_handle", "rage::internal::GetGamerHandle(int, rage::rlGamerHandle*)"),
    ("rl_offline_name", "rage::internal::GetOfflineName(int, char*, unsigned int)"),
    ("rl_pc_initialized", "rage::rlPc::IsInitialized()"),
    ("live_manager_init", "CLiveManager::Init(unsigned int)"),
    ("live_manager_update", "CLiveManager::Update(unsigned int)"),
    ("presence_online", "rage::rlPresence::IsOnline(int)"),
    // The checks on the way into freemode that Rockstar's services would
    // pass: the hardware bail, privileges, the prologue, the store, the
    // network features, the mugshot photo.
    ("bail_on_multiplayer", "CLiveManager::BailOnMultiplayer(eBailReason, int)"),
    ("ros_privilege", "rage::rlRosCredentials::HasPrivilege(rage::rlRosPrivilegeId) const"),
    ("ros_privilege_end_date", "rage::rlRosCredentials::HasPrivilegeEndDate(rage::rlRosPrivilegeId, bool*, unsigned long*) const"),
    ("profile_setting_int", "CProfileSettings::GetInt(CProfileSettings::ProfileSettingId)"),
    ("can_access_network_features", "CNetwork::CanAccessNetworkFeatures(eNetworkAccessArea, eNetworkAccessCode*)"),
    ("can_enter_multiplayer", "CNetworkSession::CanEnterMultiplayer()"),
    ("populate_store_data", "cCommerceManager::PopulateThinData(bool)"),
    ("photo_to_jpeg", "CHighQualityScreenshot::StoreScreenShotInBuffer(unsigned char*, unsigned int*, unsigned int, void*, int, int, int, unsigned int, CPhotoBuffer*)"),
    // The scripts: their bytecode patches, and natives answering about
    // Rockstar's services or content the game data lacks.
    ("script_program_loaded", "rage::scrProgram::scrProgram(rage::datResource&)"),
    ("profanity_valid", "socialclub_commands::SetupScriptCommands()::scrWrapped_SC_PROFANITY_GET_CHECK_IS_VALID::Call(rage::scrThread::Info&)"),
    ("profanity_pending", "socialclub_commands::SetupScriptCommands()::scrWrapped_SC_PROFANITY_GET_CHECK_IS_PENDING::Call(rage::scrThread::Info&)"),
    ("profanity_status", "socialclub_commands::SetupScriptCommands()::scrWrapped_SC_PROFANITY_GET_STRING_STATUS::Call(rage::scrThread::Info&)"),
    ("savemigration_mp_enabled", "savemigration_commands::SetupScriptCommands()::scrWrapped_SAVEMIGRATION_IS_MP_ENABLED::Call(rage::scrThread::Info&)"),
    ("bad_pack_order", "dlc_commands::SetupScriptCommands()::scrWrapped_GET_EVER_HAD_BAD_PACK_ORDER::Call(rage::scrThread::Info&)"),
]
