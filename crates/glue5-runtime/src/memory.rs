//! The module's linear memory: reserved once (so it never moves while other
//! threads run) and accessed by host code through [`Mem`].

use std::ffi::CStr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use anyhow::{Result, bail};
use glue5_game::{Env, MEMORY_INITIAL_PAGES, MEMORY_MAX_PAGES, Memory, PAGE_SIZE, orig};

pub struct WasmMemory {
    pub desc: *mut Memory,
    base: *mut u8,
    max_pages: u64,
}

unsafe impl Send for WasmMemory {}
unsafe impl Sync for WasmMemory {}

impl WasmMemory {
    /// Reserves address space for the largest memory the module may grow to
    /// (or the most the OS grants, but at least the initial size). Pages are
    /// committed lazily by the OS on first touch.
    pub fn reserve() -> Result<Self> {
        let mut max_pages = MEMORY_MAX_PAGES;
        loop {
            let bytes = (max_pages * PAGE_SIZE) as usize;
            if let Some(base) = map(bytes) {
                log::info!("wasm memory: {} MiB reserved at {base:p}", bytes >> 20);
                let desc = unsafe { Memory::new(base, MEMORY_INITIAL_PAGES, max_pages) };
                return Ok(WasmMemory { desc, base, max_pages });
            }
            if max_pages / 2 < MEMORY_INITIAL_PAGES {
                bail!("cannot reserve {} MiB for the wasm memory", (MEMORY_INITIAL_PAGES * PAGE_SIZE) >> 20);
            }
            max_pages /= 2;
        }
    }

    pub fn mem(&self) -> Mem {
        Mem { base: self.base }
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_pages * PAGE_SIZE
    }

    pub fn size(&self) -> u64 {
        unsafe { (*self.desc).size }
    }

    /// `emscripten_resize_heap`: memory is already reserved, growing only
    /// publishes the new size.
    pub fn grow_to(&self, bytes: u64) -> bool {
        if bytes > self.max_bytes() {
            return false;
        }
        let pages = bytes.div_ceil(PAGE_SIZE);
        unsafe {
            if pages > (*self.desc).pages {
                (*self.desc).pages = pages;
                (*self.desc).size = pages * PAGE_SIZE;
            }
        }
        true
    }
}

#[cfg(unix)]
fn map(bytes: usize) -> Option<*mut u8> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    (ptr != libc::MAP_FAILED).then_some(ptr as *mut u8)
}

#[cfg(windows)]
fn map(bytes: usize) -> Option<*mut u8> {
    // Reserve and commit lazily would need a fault handler; plain commit is fine
    // with Windows' overcommit-free model only if the page file allows it.
    let layout = std::alloc::Layout::from_size_align(bytes, 65536).ok()?;
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
    (!ptr.is_null()).then_some(ptr)
}

/// A view of wasm memory for host code. Addresses are wasm (linear memory)
/// addresses. The memory is shared with running wasm threads; plain accessors
/// are unordered, like non-atomic wasm accesses.
#[derive(Clone, Copy)]
pub struct Mem {
    base: *mut u8,
}

unsafe impl Send for Mem {}
unsafe impl Sync for Mem {}

impl Mem {
    /// # Safety
    /// `env` must be a live instance's environment.
    pub unsafe fn of(env: *mut Env) -> Mem {
        Mem { base: unsafe { (*(*env).memory).data } }
    }

    pub fn ptr(self, addr: u64) -> *mut u8 {
        unsafe { self.base.add(addr as usize) }
    }

    pub fn u8(self, addr: u64) -> u8 {
        unsafe { *self.ptr(addr) }
    }
    pub fn u16(self, addr: u64) -> u16 {
        unsafe { self.ptr(addr).cast::<u16>().read_unaligned() }
    }
    pub fn u32(self, addr: u64) -> u32 {
        unsafe { self.ptr(addr).cast::<u32>().read_unaligned() }
    }
    pub fn i32(self, addr: u64) -> i32 {
        self.u32(addr) as i32
    }
    pub fn u64(self, addr: u64) -> u64 {
        unsafe { self.ptr(addr).cast::<u64>().read_unaligned() }
    }
    pub fn f64(self, addr: u64) -> f64 {
        f64::from_bits(self.u64(addr))
    }

    pub fn set_u8(self, addr: u64, v: u8) {
        unsafe { *self.ptr(addr) = v }
    }
    pub fn set_u16(self, addr: u64, v: u16) {
        unsafe { self.ptr(addr).cast::<u16>().write_unaligned(v) }
    }
    pub fn set_u32(self, addr: u64, v: u32) {
        unsafe { self.ptr(addr).cast::<u32>().write_unaligned(v) }
    }
    pub fn set_i32(self, addr: u64, v: i32) {
        self.set_u32(addr, v as u32)
    }
    pub fn set_u64(self, addr: u64, v: u64) {
        unsafe { self.ptr(addr).cast::<u64>().write_unaligned(v) }
    }
    pub fn set_i64(self, addr: u64, v: i64) {
        self.set_u64(addr, v as u64)
    }

    pub fn bytes<'a>(self, addr: u64, len: usize) -> &'a [u8] {
        unsafe { std::slice::from_raw_parts(self.ptr(addr), len) }
    }
    #[allow(clippy::mut_from_ref)]
    pub fn bytes_mut<'a>(self, addr: u64, len: usize) -> &'a mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr(addr), len) }
    }

    /// NUL-terminated string at `addr` (lossy UTF-8).
    pub fn str(self, addr: u64) -> String {
        if addr == 0 {
            return String::new();
        }
        unsafe { CStr::from_ptr(self.ptr(addr) as *const _) }.to_string_lossy().into_owned()
    }

    /// Emscripten's `stringToUTF8`: writes at most `cap - 1` bytes plus a NUL.
    /// Returns the bytes written, without the NUL.
    pub fn write_str(self, addr: u64, s: &str, cap: usize) -> usize {
        if cap == 0 {
            return 0;
        }
        let mut n = s.len().min(cap - 1);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.bytes_mut(addr, n).copy_from_slice(&s.as_bytes()[..n]);
        self.set_u8(addr + n as u64, 0);
        n
    }

    pub fn atomic_u32<'a>(self, addr: u64) -> &'a AtomicU32 {
        unsafe { &*(self.ptr(addr) as *const AtomicU32) }
    }
    pub fn atomic_u64<'a>(self, addr: u64) -> &'a AtomicU64 {
        unsafe { &*(self.ptr(addr) as *const AtomicU64) }
    }

    /// `Atomics.store` + `Atomics.notify` on a 32-bit word.
    pub fn store_notify(self, addr: u64, v: u32) {
        let word = self.atomic_u32(addr);
        word.store(v, Ordering::SeqCst);
        glue5_game::futex::notify(word, u32::MAX);
    }
}

/// The process's memory footprint (what iOS limits) and, on iOS, how much
/// more it may take before the system terminates it, in bytes.
pub fn footprint() -> (Option<u64>, Option<u64>) {
    #[cfg(target_vendor = "apple")]
    let used = unsafe {
        // libproc: in the system on iOS too, though its header is macOS-only.
        unsafe extern "C" {
            fn proc_pid_rusage(pid: libc::c_int, flavor: libc::c_int, buffer: *mut libc::c_void) -> libc::c_int;
        }
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let ok = proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, &mut info as *mut _ as *mut libc::c_void) == 0;
        ok.then_some(info.ri_phys_footprint)
    };
    #[cfg(not(target_vendor = "apple"))]
    let used = None;
    #[cfg(target_os = "ios")]
    let available = {
        unsafe extern "C" {
            fn os_proc_available_memory() -> usize;
        }
        Some(unsafe { os_proc_available_memory() } as u64)
    };
    #[cfg(not(target_os = "ios"))]
    let available = None;
    (used, available)
}

// ---- the engine's resource heap ----------------------------------------------

/// The resource heap's size at most, in bytes (0: no limit).
static RESOURCE_CAP: AtomicU64 = AtomicU64::new(0);

/// Caps the resource heap at `mb` MiB (the engine's `gameResourceMB`).
pub(crate) fn cap_resource_heap(mb: u32) {
    RESOURCE_CAP.store((mb as u64) << 20, Ordering::Relaxed);
}

/// Hook for `rage::sysMemGrowBuddyAllocator::GrowHeap()`. The PC build's
/// resource heap starts at `gameResourceMB` and grows by up to 256 MiB
/// whenever the streaming engine asks how much memory is left (answering
/// grows it), so it holds whatever the player has passed by: on a phone,
/// past what the system allows. Capped, the streaming engine finds the heap
/// full and evicts, as on consoles. -1: no growth.
pub(crate) unsafe extern "C" fn grow_heap(env: *mut Env, this: u64) -> u32 {
    // Heaps come a little short of whole MiB (255, 511, ...): within the
    // smallest growth step of the cap counts as there.
    const STEP: u64 = 16 << 20;
    let cap = RESOURCE_CAP.load(Ordering::Relaxed);
    if cap != 0 && unsafe { orig::heap_size(env, this) } + STEP > cap {
        return u32::MAX;
    }
    unsafe { orig::grow_heap(env, this) }
}
