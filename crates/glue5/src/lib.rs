//! Glue 5: the game as one application for every platform — desktop (macOS,
//! Linux, Windows) and mobile (iOS; Android next).
//!
//! The engine runs on its own threads ([`glue5_runtime`]) and renders with
//! [`glue5_gpu`]; this crate is the platform side, through SDL: window,
//! input, audio and the application's own settings. One entry point serves
//! every platform:
//!
//! - desktop: the `glue5` binary calls [`main`];
//! - iOS: the Xcode project's `main` calls `glue5_main`, which hands the
//!   process to SDL (`SDL_RunApp` / `UIApplicationMain`);
//! - Android: SDL's Java activity loads the library and calls `SDL_main`.

mod app;
mod audio;
mod keymap;
mod pads;
mod paths;
mod settings;
mod touch;
mod ui;

use sdl3::messagebox::{MessageBoxFlag, show_simple_message_box};

/// Runs the application; returns the process exit code.
pub fn main() -> i32 {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match app::run() {
        Ok(()) => 0,
        Err(e) => {
            log::error!("{e:#}");
            let _ = show_simple_message_box(MessageBoxFlag::ERROR, "Glue 5", &format!("{e:#}"), None);
            1
        }
    }
}

#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn glue5_main(argc: std::ffi::c_int, argv: *mut *mut std::ffi::c_char) -> std::ffi::c_int {
    unsafe extern "C" fn sdl_main(_: std::ffi::c_int, _: *mut *mut std::ffi::c_char) -> std::ffi::c_int {
        main()
    }
    unsafe { sdl3::sys::main::SDL_RunApp(argc, argv, Some(sdl_main), std::ptr::null_mut()) }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn SDL_main(_: std::ffi::c_int, _: *mut *mut std::ffi::c_char) -> std::ffi::c_int {
    main()
}
