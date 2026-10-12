//! The game module, recompiled to native code by the build script.
//!
//! This crate is the raw boundary: the host supplies [`Imports`] (one function
//! per wasm import) and optional [`Hooks`], creates the shared [`Memory`], and
//! runs one instance per engine thread, each with its own [`Env`]. Calls into
//! the module go through [`exports`], which turn wasm traps into [`Trap`].

#![allow(non_snake_case, non_upper_case_globals, clippy::missing_safety_doc)]

#[cfg(test)]
#[path = "../build/mangle.rs"]
mod mangle;

use std::ffi::{CStr, c_char, c_void};
use std::fmt;

pub mod futex;

mod bindings {
    use super::{Env, Trap};
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub use bindings::{Hooks, Imports, MEMORY_INITIAL_PAGES, MEMORY_MAX_PAGES, exports, orig};

/// The game build this crate was generated from (`b/<build>` directory name).
pub const GAME_BUILD: &str = env!("GLUE5_GAME_BUILD");

/// Size of a wasm page.
pub const PAGE_SIZE: u64 = 65536;

/// Opaque wasm2c instance (`w2c_game`).
#[repr(C)]
pub struct Instance {
    _private: [u8; 0],
}

/// Per-instance environment handed to every import and hook (`struct w2c_env`).
#[repr(C)]
pub struct Env {
    /// The module's shared memory.
    pub memory: *mut Memory,
    /// This thread's instance.
    pub instance: *mut Instance,
    /// Host data for this thread.
    pub host: *mut c_void,
    longjmp_target: *mut c_void,
}

impl Env {
    pub const fn new(memory: *mut Memory, host: *mut c_void) -> Self {
        Env { memory, instance: std::ptr::null_mut(), host, longjmp_target: std::ptr::null_mut() }
    }
}

/// The memory's base for the generated code, which declares it const
/// (`WASM_RT_NONCONFORMING_FIXED_MEMORY_BASE`): set once, by [`Memory::new`],
/// before any instance exists.
#[unsafe(no_mangle)]
static mut wasm_rt_fixed_memory_base: *mut u8 = std::ptr::null_mut();

/// `wasm_rt_shared_memory_t`. Only the leading fields are mirrored; allocate it
/// with [`Memory::new`].
#[repr(C)]
pub struct Memory {
    pub data: *mut u8,
    pub data_end: *mut u8,
    pub page_size: u32,
    pub pages: u64,
    pub max_pages: u64,
    pub size: u64,
    // followed by is64 and a mutex
}

impl Memory {
    /// Describes host-owned memory: `data` must stay valid (and must not move)
    /// for `max_pages` pages for the rest of the process. The descriptor itself
    /// is never freed.
    /// The module has one memory: this may be called once per process.
    pub unsafe fn new(data: *mut u8, pages: u64, max_pages: u64) -> *mut Memory {
        unsafe {
            let base = &raw mut wasm_rt_fixed_memory_base;
            assert!(base.read().is_null(), "the module's memory exists already");
            base.write(data);
            let size = glue5_memory_struct_size();
            let layout = std::alloc::Layout::from_size_align(size, 16).unwrap();
            let memory = std::alloc::alloc_zeroed(layout) as *mut Memory;
            glue5_memory_init(memory, data, pages, max_pages);
            memory
        }
    }
}

/// A wasm trap (wasm2c `wasm_rt_trap_t`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Trap(pub u32);

impl Trap {
    pub fn check(code: u32) -> Result<(), Trap> {
        if code == 0 { Ok(()) } else { Err(Trap(code)) }
    }
}

impl fmt::Debug for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == UNWIND {
            return f.write_str("unwind");
        }
        let msg = unsafe { CStr::from_ptr(wasm_rt_strerror(self.0)) };
        write!(f, "wasm trap: {}", msg.to_string_lossy())
    }
}

impl std::error::Error for Trap {}

/// What a call into wasm returns when the code asked to unwind while keeping
/// the runtime alive (`emscripten_exit_with_live_runtime`).
pub const UNWIND: Trap = Trap(0x7f000001);

/// Unwinds to the innermost call into wasm, which then fails with [`UNWIND`].
/// Only call it from an import or hook.
pub fn unwind() -> ! {
    unsafe { glue5_unwind() }
}

/// Installs the host. Call once, before creating any instance.
pub fn install(imports: Imports, hooks: Hooks) {
    unsafe {
        glue5_init();
        glue5_set_atomics(futex::wasm_wait32, futex::wasm_wait64, futex::wasm_notify);
        *std::ptr::addr_of_mut!(bindings::glue5_imports_table) = imports;
        *std::ptr::addr_of_mut!(bindings::glue5_hooks_table) = hooks;
    }
}

/// Creates the calling thread's instance in `env` and runs the start function.
pub unsafe fn instantiate(env: *mut Env) -> Result<(), Trap> {
    Trap::check(unsafe { glue5_instance_new(env) })
}

/// Calls table function `index` as `void* (*)(void*)`, e.g. a pthread start routine.
pub unsafe fn call_ptr_ptr(env: *mut Env, index: u64, arg: u64) -> Result<u64, Trap> {
    let mut out = 0;
    Trap::check(unsafe { glue5_call_ptr_ptr(env, index, arg, &mut out) }).map(|()| out)
}

/// Frees `env`'s instance.
pub unsafe fn free_instance(env: *mut Env) {
    unsafe { glue5_instance_free(env) }
}

unsafe extern "C" {
    fn glue5_init();
    fn glue5_set_atomics(
        wait32: unsafe extern "C" fn(*mut Memory, u64, u32, i64) -> u32,
        wait64: unsafe extern "C" fn(*mut Memory, u64, u64, i64) -> u32,
        notify: unsafe extern "C" fn(*mut Memory, u64, u32) -> u32,
    );
    fn glue5_call_ptr_ptr(env: *mut Env, index: u64, arg: u64, out: *mut u64) -> u32;
    fn glue5_memory_struct_size() -> usize;
    fn glue5_memory_init(memory: *mut Memory, data: *mut u8, pages: u64, max_pages: u64);
    fn glue5_instance_new(env: *mut Env) -> u32;
    fn glue5_instance_free(env: *mut Env);
    fn wasm_rt_strerror(trap: u32) -> *const c_char;
    fn glue5_unwind() -> !;
    /// wasm2c runtime (patches/wabt/0005).
    static mut wasm_rt_trap_hook: Option<unsafe extern "C" fn(u32, u32)>;
}

/// Calls `hook` with the reason where a trap occurs, and for `unreachable`
/// the line of the generated C (0 otherwise), before execution unwinds out
/// of the module: the trapping call stack is still there.
pub fn set_trap_hook(hook: unsafe extern "C" fn(u32, u32)) {
    // SAFETY: set once, before the module runs.
    unsafe { wasm_rt_trap_hook = Some(hook) };
}
