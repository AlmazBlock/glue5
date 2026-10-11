// Game functions the host can take over.
//
// Each entry pairs a field name with the function's name in the wasm name
// section. The build script finds the function, derives its signature from the
// module and routes every call to it through `Hooks::<field>`. An unset hook
// falls back to the original, and a hook can call the original through
// `orig::<field>`. Hooks receive the same `*mut Env` as imports. An entry
// the host leaves unset gives it a way to call that game function.
//
// This file is included by the build script.

pub const HOOKS: &[(&str, &str)] = &[
    // Once per frame for each of the 4 pads: feeds native gamepads (this build
    // has no XInput and never fills the DirectInput stick table).
    ("pad_update", "rage::ioPad::Update(bool)"),
    // The game's text box (save names, the phone's cheat entry, ...): touch
    // devices show the system keyboard while it is open.
    ("text_box_open", "CTextInputBox::Open(rage::ioVirtualKeyboard::Params const&)"),
    ("text_box_close", "CTextInputBox::Close()"),
    // The engine's way out: asked by the main loop whether to leave it and
    // shut down ("Exit Game"), and the first step of the restart some
    // graphics settings need. The shutdown never finishes in this port; the
    // host ends the process instead.
    ("want_to_exit", "CSystem::WantToExit()"),
    ("rl_shutdown", "rage::rlShutdown()"),
    // The resource heap grows whenever the streaming engine asks how much
    // memory is left; on low-memory devices the host lets it grow only for
    // allocations (and only calls GetHeapSize, to know the heap's size).
    ("grow_heap", "rage::sysMemGrowBuddyAllocator::GrowHeap()"),
    ("heap_size", "rage::sysMemGrowBuddyAllocator::GetHeapSize() const"),
    ("memory_available", "rage::sysMemGrowBuddyAllocator::GetMemoryAvailable()"),
    ("largest_block", "rage::sysMemGrowBuddyAllocator::GetLargestAvailableBlock()"),
    // At PostFX Ultra the skin pass asks for a pass the game's shaders do not
    // have; it is shown settings with PostFX at Very High (see skin.rs).
    ("skin_light_pass", "DeferredLighting::ExecuteSkinLightPass(rage::grcRenderTarget*, bool)"),
    ("get_settings", "CSettingsManager::GetSettings()"),
    // MSAA stays off: the game's shaders cannot draw it (see msaa.rs). The
    // settings loaded from the file and those the menu asks for; the
    // manager, for the host.
    ("settings_load", "CSettingsManager::Load()"),
    ("settings_request", "CSettingsManager::RequestNewSettings(Settings)"),
    ("settings_instance", "CSettingsManager::GetInstance()"),
    // A device reset (the window resized) frees staging memory a readback
    // may still be writing and the depth target the 2D sprite shader may
    // still hold: the host waits for the GPU first and takes the depth
    // target away after (device_reset.rs).
    ("device_lost", "rage::grcDevice::DeviceLost()"),
    ("gpu_fence", "wasm_gpu::Fence()"),
    ("sprite_depth_texture", "CSprite2d::SetDepthTexture(rage::grcTexture*)"),
    // Any resource, at any time: its destructors free its CPU memory before
    // waiting for a readback into it; the host waits first (readback.rs).
    // The wait, then each resource type's destructor and deleting destructor.
    ("wait_readback", "wasm_null_d3d::HasGpuId::WaitReadback()"),
    ("buffer_dtor", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Buffer, D3D11_BUFFER_DESC>::~NullResource()"),
    ("buffer_delete", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Buffer, D3D11_BUFFER_DESC>::~NullResource()_567"),
    ("texture1d_dtor", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Texture1D, D3D11_TEXTURE1D_DESC>::~NullResource()"),
    ("texture1d_delete", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Texture1D, D3D11_TEXTURE1D_DESC>::~NullResource()_586"),
    ("texture2d_dtor", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Texture2D, D3D11_TEXTURE2D_DESC>::~NullResource()"),
    ("texture2d_delete", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Texture2D, D3D11_TEXTURE2D_DESC>::~NullResource()_472"),
    ("texture3d_dtor", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Texture3D, D3D11_TEXTURE3D_DESC>::~NullResource()"),
    ("texture3d_delete", "wasm_null_d3d::NullResource<wasm_null_d3d::Stub_ID3D11Texture3D, D3D11_TEXTURE3D_DESC>::~NullResource()_605"),
];
