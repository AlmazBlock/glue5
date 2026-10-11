//! The engine's graphics settings: `settings.xml` in the user directory,
//! which its pause menu edits and saves (Settings › Graphics). Changes apply
//! at once, or, for those that need it, at a restart the engine asks for
//! (see [`crate::exit`]).
//!
//! The host never overrides them on the command line, which would undo the
//! menu's choices at every start: it only writes the low-memory preset over
//! them, once.

use std::io;
use std::path::{Path, PathBuf};

fn path(user_dir: &Path) -> PathBuf {
    user_dir.join("Documents/Rockstar Games/GTA V/settings.xml")
}

/// The low-memory preset (the browser build's `?low=1`): the lowest texture,
/// reflection, particle and grass quality, shadows at the lowest level,
/// minimum ped and vehicle variety, city density and level of detail.
/// Measured in a Node run at the world: GPU memory 1.16 GB -> 0.5 GB,
/// streamed assets 1.38 GB -> 0.95 GB.
const LOW: &[(&str, &str)] = &[
    ("TextureQuality", "0"),
    ("ShadowQuality", "1"),
    ("ReflectionQuality", "0"),
    ("ParticleQuality", "0"),
    ("GrassQuality", "0"),
    ("CityDensity", "0.000000"),
    ("PedVarietyMultiplier", "0.000000"),
    ("VehicleVarietyMultiplier", "0.000000"),
    ("LodScale", "0.000000"),
    ("PedLodBias", "0.000000"),
    ("VehicleLodBias", "0.000000"),
];

/// What the engine writes when it first starts without settings (its
/// defaults for the adapter it is given, the same everywhere).
const DEFAULTS: &str = include_str!("settings.xml");

/// Writes the low-memory preset over the current settings (or the engine's
/// defaults, before its first start).
pub fn apply_low_preset(user_dir: &Path) -> io::Result<()> {
    let path = path(user_dir);
    let mut xml = std::fs::read_to_string(&path).unwrap_or_else(|_| DEFAULTS.to_string());
    for (name, value) in LOW {
        xml = set(&xml, name, value);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, xml)
}

/// `xml` with `<name value="..." />` set to `value` (unchanged if absent).
fn set(xml: &str, name: &str, value: &str) -> String {
    let tag = format!("<{name} value=\"");
    let Some(start) = xml.find(&tag).map(|i| i + tag.len()) else { return xml.to_string() };
    let Some(len) = xml[start..].find('"') else { return xml.to_string() };
    format!("{}{value}{}", &xml[..start], &xml[start + len..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_values_by_exact_name() {
        let xml = "<LodScale value=\"1.000000\" />\n<MaxLodScale value=\"1.000000\" />";
        assert_eq!(
            set(xml, "LodScale", "0.000000"),
            "<LodScale value=\"0.000000\" />\n<MaxLodScale value=\"1.000000\" />"
        );
        assert_eq!(set(xml, "Missing", "0"), xml);
    }

    #[test]
    fn preset_names_are_in_the_defaults() {
        for (name, _) in LOW {
            assert!(DEFAULTS.contains(&format!("<{name} value=\"")), "{name}");
        }
    }
}
