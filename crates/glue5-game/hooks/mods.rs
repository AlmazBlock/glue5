// Hooks for glue5-mods (`crates/glue5-mods/src/engine.rs`; see ../hooks.rs).
&[
    // Each mod runs in a script thread of its own whose program only waits,
    // in a loop: every frame the engine calls WAIT for it, and the hook runs
    // the mod's Lua there, inside a real script context.
    ("script_wait", "rage::scrThread::Wait(rage::scrThread::Info&)"),
    ("script_active_thread", "rage::scrThread::GetActiveThread()"),
    ("script_get_thread", "rage::scrThread::GetThread(rage::scrThreadId)"),
    ("script_kill_thread", "rage::scrThread::KillThread(rage::scrThreadId)"),
    ("script_get_program", "rage::scrProgram::GetProgram(rage::scrProgramId)"),
    ("script_insert_program", "scrProgramRegistry::Insert(rage::scrProgram*)"),
    (
        "script_start_program",
        "CTheScripts::GtaStartNewThreadWithProgramId(rage::scrProgramId, void const*, int, int, char const*)",
    ),
]
