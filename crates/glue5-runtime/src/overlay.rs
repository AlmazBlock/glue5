//! Game files changed on their way to the engine, where its code is a later
//! revision than the game's data.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

type Patch = fn(String) -> Option<String>;

/// The files (under the game's `data/`), whether this run changes them, and
/// how.
const OVERLAYS: [(&str, fn() -> bool, Patch); 2] = [
    ("common/data/gameconfig.xml", online, crate::online::more_broadcast_data),
    ("common/data/ui/pausemenu.xml", always, autosave_first),
];

fn online() -> bool {
    crate::rt().config.online.is_some()
}

fn always() -> bool {
    true
}

/// Where the file at `rel` (under the game's `data/`) is changed, the changed
/// copy (made once, in `scratch`). Same length as the original: the engine
/// may size its reads from the game's manifest.
pub(crate) fn game_file(rel: &str, real: &Path, scratch: &Path) -> Option<PathBuf> {
    static MADE: Mutex<Vec<(&str, Option<PathBuf>)>> = Mutex::new(Vec::new());
    let (name, _, patch) = OVERLAYS.iter().find(|(name, applies, _)| rel.eq_ignore_ascii_case(name) && applies())?;
    let mut made = MADE.lock().unwrap();
    if let Some((_, path)) = made.iter().find(|(n, _)| n == name) {
        return path.clone();
    }
    let path = (|| {
        let text = std::fs::read_to_string(real).ok()?;
        let patched = patch(text.clone()).filter(|p| p.len() == text.len())?;
        let path = scratch.join("overlay").join(real.file_name()?);
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, patched).ok()?;
        log::info!("overlay: {rel} changed");
        Some(path)
    })();
    made.push((name, path.clone()));
    path
}

/// pausemenu.xml, "Saving And Startup": the engine greys "Startup Flow" out
/// while the landing page preference is on, which later data has an item
/// for; this data has none, so it stays on. With its first item greyed out,
/// the list sends left and right to that item whichever one is highlighted,
/// and "Autosave" could not be turned off. Autosave goes first.
fn autosave_first(text: String) -> Option<String> {
    let line = |pref: &str| {
        let at = text.find(&format!("<MenuPref>{pref}</MenuPref>"))?;
        let start = text[..at].rfind('\n')? + 1;
        Some(start..at + text[at..].find('\n')? + 1)
    };
    let (flow, autosave) = (line("PREF_STARTUP_FLOW")?, line("PREF_AUTOSAVE")?);
    (flow.end == autosave.start)
        .then(|| format!("{}{}{}{}", &text[..flow.start], &text[autosave.clone()], &text[flow], &text[autosave.end..]))
}

#[test]
fn autosave_goes_first() {
    let text = "<a>\r\n\t<Item> <MenuPref>PREF_STARTUP_FLOW</MenuPref> </Item>\r\n\t<Item> <MenuPref>PREF_AUTOSAVE</MenuPref> </Item>\r\n</a>\r\n";
    let want = "<a>\r\n\t<Item> <MenuPref>PREF_AUTOSAVE</MenuPref> </Item>\r\n\t<Item> <MenuPref>PREF_STARTUP_FLOW</MenuPref> </Item>\r\n</a>\r\n";
    assert_eq!(autosave_first(text.into()).as_deref(), Some(want));
}
