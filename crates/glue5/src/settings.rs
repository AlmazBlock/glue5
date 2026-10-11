//! The application's own settings, in `glue5.json` in the user directory.
//! Graphics quality is the game's (its menu, Settings › Graphics, saved in
//! its `settings.xml`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::MOBILE;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The game directory, once found or chosen.
    pub game_dir: Option<PathBuf>,
    /// Smaller engine pools, and the game's graphics settings lowered once
    /// (the browser build's `?low=1`).
    pub low_memory: bool,
    /// Whether the low-memory preset is in the game's settings (written at
    /// the first start with `low_memory`; the game's menu changes it after).
    pub low_preset_written: bool,
    /// The game's resolution as a fraction of the window's, per side (the
    /// back buffer follows the window). Chosen for the screen at the first
    /// start ([`Settings::for_screen`]).
    pub render_scale: Option<f32>,
    pub vsync: bool,
    /// At most this many frames per second; 0: no limit of the engine's
    /// (the display's refresh rate with VSync).
    pub max_fps: u32,
    /// MetalFX upscaling, where the GPU has it (Apple).
    pub metalfx: bool,
    pub touch: TouchMode,
    pub touch_opacity: f32,
    pub touch_size: f32,
    /// Settings saved before the render scale: the back buffer's height.
    #[serde(skip_serializing)]
    render_height: Option<u32>,
}

/// When the on-screen controls show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchMode {
    /// On touch screens, while no controller (or keyboard) is in use.
    Auto,
    Always,
    Off,
}

/// Phones and tablets with less RAM start with the low-memory profile (the
/// game takes about 4 GB at the normal one).
const LOW_MEMORY_RAM_MB: i32 = 7000;

/// The render scale's range, in steps of 5%.
pub const RENDER_SCALES: std::ops::RangeInclusive<f32> = 0.5..=1.0;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            game_dir: None,
            low_memory: MOBILE && sdl3::cpuinfo::system_ram() < LOW_MEMORY_RAM_MB,
            low_preset_written: false,
            render_scale: None,
            vsync: true,
            // Phones hold a steady 60 for longer than they hold 120 (heat).
            max_fps: if MOBILE { 60 } else { 0 },
            metalfx: true,
            touch: TouchMode::Auto,
            touch_opacity: 0.6,
            touch_size: 1.0,
            render_height: None,
        }
    }
}

impl Settings {
    fn file(user_dir: &Path) -> PathBuf {
        user_dir.join("glue5.json")
    }

    pub fn load(user_dir: &Path) -> Settings {
        match std::fs::read(Self::file(user_dir)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("ignoring unreadable settings: {e}");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, user_dir: &Path) {
        let json = serde_json::to_vec_pretty(self).expect("settings serialise");
        if let Err(e) = std::fs::write(Self::file(user_dir), json) {
            log::warn!("cannot save settings: {e}");
        }
    }

    /// Picks the render scale, the first time, for a window `height` pixels
    /// tall: the height an older settings file had, else about 1080 lines on
    /// desktop and 720 on phones and tablets (what the game ran at before:
    /// its GPU time grows with the pixels, and phone screens have many).
    pub fn for_screen(&mut self, height: u32) {
        if self.render_scale.is_none() {
            let lines = self.render_height.take().unwrap_or(if MOBILE { 720 } else { 1080 });
            self.render_scale = Some(round_scale(lines as f32 / height.max(1) as f32));
        }
    }

    pub fn render_scale(&self) -> f32 {
        self.render_scale.unwrap_or(1.0)
    }
}

/// A render scale in the range, to its 5% step.
pub fn round_scale(scale: f32) -> f32 {
    ((scale * 20.0).round() / 20.0).clamp(*RENDER_SCALES.start(), *RENDER_SCALES.end())
}

/// The game's back buffer for a window of `pixels` at `scale`, as the
/// browser build sizes it: even sides (the engine halves them for its
/// post-processing), at least the 320x240 the engine takes, at most 4K
/// (1440p with low memory: render targets the size of the screen are most
/// of the GPU's memory).
pub fn render_size((w, h): (u32, u32), scale: f32, low_memory: bool) -> (u32, u32) {
    let cap = if low_memory { (2560.0, 1440.0) } else { (3840.0, 2160.0) };
    let (w, h) = (w as f32 * scale, h as f32 * scale);
    let k = (cap.0 / w).min(cap.1 / h).min(1.0);
    let side = |v: f32, min: u32| ((v * k / 2.0).round() as u32 * 2).max(min);
    (side(w, 320), side(h, 240))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_sizes() {
        assert_eq!(render_size((2560, 1440), 0.75, false), (1920, 1080));
        assert_eq!(render_size((2532, 1170), 0.6, false), (1520, 702));
        assert_eq!(render_size((300, 100), 0.5, false), (320, 240));
        assert_eq!(render_size((6016, 3384), 1.0, false), (3840, 2160));
        assert_eq!(render_size((6016, 3384), 1.0, true), (2560, 1440));
    }

    #[test]
    fn first_scale() {
        let mut s = Settings { render_height: Some(720), ..Settings::default() };
        s.for_screen(1440);
        assert_eq!(s.render_scale, Some(0.5));
        s.for_screen(720);
        assert_eq!(s.render_scale, Some(0.5), "chosen once");
        let mut s = Settings::default();
        s.for_screen(1170);
        assert_eq!(s.render_scale, Some(if MOBILE { 0.6 } else { 0.9 }));
    }
}
