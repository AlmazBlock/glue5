//! Desktop entry point: `glue5 [game dir]`.

/// Windows' heap hands large blocks back to the system when they are freed,
/// and the renderer allocates and frees many every frame (wgpu-core's
/// trackers for each command buffer, the frame's buffers): each frame then
/// pays page faults and zeroing again. mimalloc keeps them for reuse.
#[cfg(windows)]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    std::process::exit(glue5::main());
}
