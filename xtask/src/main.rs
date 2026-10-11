//! Developer tasks: `cargo xtask <command>`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

const USAGE: &str = "\
usage: cargo xtask <command>

  setup              prepare the third-party code with our patches: wgpu-core
                     (from crates.io) and wasm2c (built from third_party/wabt);
                     once, and again after patches/ change
  ios [device]       build the iOS app, install it on the device and launch it
                     with its console here (signed with team GLUE5_TEAM;
                     device: name or id, default the only one connected)
  ios-build          only build the iOS app, unsigned (checks that it links)
  ios-run [device]   launch the installed app again, with its console
  ios-game [device]  copy the game (GLUE5_GAME_DIR: data/ and the shaders)
                     into the app's Documents/game; unchanged files are
                     skipped, so an interrupted copy resumes

The app on the device gets RUST_LOG and the GLUE5_* variables set here.
";

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let device = args.get(1).map(String::as_str);
    match args.first().map(String::as_str) {
        Some("setup") => setup(),
        Some("ios") => ios(device),
        Some("ios-build") => ios_build().map(|app| println!("built {} (unsigned)", app.display())),
        Some("ios-run") => ios_run(&connected_device(device)?.0),
        Some("ios-game") => ios_game(device),
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Where tools built by `setup` live. glue5-game/build.rs looks in the same place.
fn tools_dir() -> PathBuf {
    let target = env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| workspace_root().join("target"));
    target.join("glue5-tools")
}

fn setup() -> Result<()> {
    let root = workspace_root();
    setup_wgpu_core(&root)?;
    setup_wasm2c(&root)
}

/// The wgpu-core release the patches in patches/wgpu-core apply to: its
/// version and the sha256 of its crates.io archive.
const WGPU_CORE: (&str, &str) = ("30.0.1", "14c018fce9b6270aa203c2fdd56f3cce996713534bd757e4ea58c8560b121f14");

/// wgpu-core from crates.io with our patches, in third_party/wgpu-core
/// (ignored by git), where the workspace's `[patch.crates-io]` takes it.
fn setup_wgpu_core(root: &Path) -> Result<()> {
    let (version, sha) = WGPU_CORE;
    let dest = root.join("third_party/wgpu-core");
    let patches = patches(root, "wgpu-core")?;
    let key = format!("{version} {sha}{}", patches_key(&patches)?);
    let stamp = dest.join(".glue5-stamp");
    if fs::read_to_string(&stamp).ok().as_deref() == Some(key.as_str()) {
        println!("wgpu-core is up to date ({})", dest.display());
        return Ok(());
    }

    let tools = tools_dir();
    fs::create_dir_all(&tools)?;
    let archive = tools.join(format!("wgpu-core-{version}.crate"));
    if sha256(&archive).ok().as_deref() != Some(sha) {
        let url = format!("https://static.crates.io/crates/wgpu-core/wgpu-core-{version}.crate");
        run(Command::new("curl").args(["-fsSL", "-o"]).arg(&archive).arg(url))?;
        ensure!(sha256(&archive)? == sha, "{} does not match its checksum", archive.display());
    }

    // Unpacked and patched next to its place, then moved there.
    let tmp = root.join("third_party/wgpu-core.tmp");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    run(Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&tmp))?;
    let src = tmp.join(format!("wgpu-core-{version}"));
    for patch in &patches {
        apply(&src, patch, &tmp)?;
    }
    fs::write(src.join(".glue5-stamp"), key)?;
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&src, &dest)?;
    fs::remove_dir_all(&tmp)?;
    println!("prepared {}", dest.display());
    Ok(())
}

/// wabt's sources at the pinned commit (third_party/wabt) with our patches,
/// built into the wasm2c the game's build script uses.
fn setup_wasm2c(root: &Path) -> Result<()> {
    let wabt = root.join("third_party/wabt");

    // Only the nested submodules wasm2c needs (wabt also pulls large test suites).
    run(Command::new("git").current_dir(root).args([
        "submodule",
        "update",
        "--init",
        "--depth",
        "1",
        "third_party/wabt",
    ]))?;
    run(Command::new("git").current_dir(&wabt).args([
        "submodule",
        "update",
        "--init",
        "--depth",
        "1",
        "third_party/picosha2",
        "third_party/simde",
    ]))?;

    let tools = tools_dir();
    let src = tools.join("wabt-src");
    let build = tools.join("wabt-build");
    let stamp = tools.join("wasm2c.stamp");
    let patches = patches(root, "wabt")?;
    let head = Command::new("git").current_dir(&wabt).args(["rev-parse", "HEAD"]).output()?;
    ensure!(head.status.success(), "git rev-parse failed in third_party/wabt");
    let key = format!("{}{}", String::from_utf8(head.stdout)?.trim(), patches_key(&patches)?);
    if fs::read_to_string(&stamp).ok().as_deref() == Some(key.as_str()) && tools.join("bin/wasm2c").exists() {
        println!("wasm2c is up to date ({})", tools.join("bin/wasm2c").display());
        return Ok(());
    }

    // Pristine copy of the pinned sources, then our patches on top.
    let _ = fs::remove_dir_all(&src);
    fs::create_dir_all(&src)?;
    let archive = Command::new("git")
        .current_dir(&wabt)
        .args(["archive", "--format=tar", "HEAD"])
        .stdout(Stdio::piped())
        .spawn()?;
    run(Command::new("tar").args(["-x", "-C"]).arg(&src).stdin(archive.stdout.context("git archive")?))?;
    copy_dir(&wabt.join("third_party/picosha2"), &src.join("third_party/picosha2"))?;
    for patch in &patches {
        apply(&src, patch, &tools)?;
    }

    // Host tool: make sure an iOS build environment (e.g. from Xcode) does not leak in.
    let mut configure = Command::new("cmake");
    configure.arg("-S").arg(&src).arg("-B").arg(&build).args([
        "-DCMAKE_BUILD_TYPE=Release",
        "-DBUILD_TESTS=OFF",
        "-DBUILD_LIBWASM=OFF",
        "-DWITH_WASI=OFF",
        "-DBUILD_TOOLS=ON",
    ]);
    for var in ["SDKROOT", "IPHONEOS_DEPLOYMENT_TARGET", "CC", "CXX", "CFLAGS", "CXXFLAGS"] {
        configure.env_remove(var);
    }
    run(&mut configure)?;
    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    run(Command::new("cmake").arg("--build").arg(&build).args([
        "--target",
        "wasm2c",
        "--parallel",
        &jobs.to_string(),
    ]))?;

    fs::create_dir_all(tools.join("bin"))?;
    fs::copy(build.join("wasm2c"), tools.join("bin/wasm2c"))?;
    fs::write(&stamp, key)?;
    println!("built {}", tools.join("bin/wasm2c").display());
    Ok(())
}

/// Our patches for a third-party project: patches/<name>/*.patch, in order.
fn patches(root: &Path, name: &str) -> Result<Vec<PathBuf>> {
    let mut list: Vec<PathBuf> =
        fs::read_dir(root.join("patches").join(name))?.map(|e| e.map(|e| e.path())).collect::<std::io::Result<_>>()?;
    list.retain(|p| p.extension().is_some_and(|e| e == "patch"));
    list.sort();
    Ok(list)
}

/// Changes when any of the patches changes.
fn patches_key(patches: &[PathBuf]) -> Result<String> {
    let mut key = String::new();
    for patch in patches {
        let sum = fs::read(patch)?.iter().fold(0u64, |h, &b| h.wrapping_mul(0x100000001b3) ^ b as u64);
        key += &format!(" {}:{sum:016x}", patch.file_name().unwrap().to_string_lossy());
    }
    Ok(key)
}

/// Applies `patch` to the files in `dir`. The copies sit inside this
/// repository: git must not discover it (above `ceiling`), or `git apply`
/// resolves paths against our root and silently skips everything.
fn apply(dir: &Path, patch: &Path, ceiling: &Path) -> Result<()> {
    println!("applying {}", patch.file_name().unwrap().to_string_lossy());
    run(Command::new("git").current_dir(dir).env("GIT_CEILING_DIRECTORIES", ceiling).arg("apply").arg(patch))
}

fn sha256(path: &Path) -> Result<String> {
    let digest = Sha256::digest(fs::read(path)?);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            if entry.file_name() != ".git" {
                copy_dir(&entry.path(), &dest)?;
            }
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed with {status}");
    }
    Ok(())
}

const IOS_BUNDLE_ID: &str = "local.glue5";
const IOS_TARGET: &str = "aarch64-apple-ios";

/// The iOS deployment target, as in platform/ios/project.yml. Only for iOS
/// builds: in the environment of macOS builds it breaks SDL's CMake build.
const IOS_DEPLOYMENT_TARGET: &str = "16.4";

/// Builds the app's library (the glue5 crate as a static library) and
/// generates the Xcode project from platform/ios/project.yml, next to it.
fn ios_project() -> Result<PathBuf> {
    run(Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(workspace_root())
        .env("IPHONEOS_DEPLOYMENT_TARGET", IOS_DEPLOYMENT_TARGET)
        .args(["rustc", "--release", "-p", "glue5", "--lib", "--crate-type", "staticlib", "--target", IOS_TARGET]))?;
    let dir = workspace_root().join("platform/ios");
    run(Command::new("xcodegen").args(["generate", "--quiet", "--spec"]).arg(dir.join("project.yml")))
        .context("xcodegen (brew install xcodegen)")?;
    Ok(dir.join("Glue5.xcodeproj"))
}

fn xcodebuild(project: &Path, destination: &str, extra: &[&str]) -> Result<PathBuf> {
    let derived = workspace_root().join("target/ios/derived");
    run(Command::new("xcodebuild")
        .arg("-project")
        .arg(project)
        .args(["-scheme", "Glue5", "-configuration", "Release", "-sdk", "iphoneos", "-destination", destination])
        .arg("-quiet")
        .arg("-derivedDataPath")
        .arg(&derived)
        .args(extra)
        .arg("build"))?;
    Ok(derived.join("Build/Products/Release-iphoneos/Glue 5.app"))
}

fn ios_build() -> Result<PathBuf> {
    xcodebuild(&ios_project()?, "generic/platform=iOS", &["CODE_SIGNING_ALLOWED=NO"])
}

fn ios(device: Option<&str>) -> Result<()> {
    if env::var_os("GLUE5_TEAM").is_none() {
        bail!("set GLUE5_TEAM to your Apple development team id (Xcode > Settings > Accounts)");
    }
    let (device, udid) = connected_device(device)?;
    // Built for the device itself: Xcode registers it with the team and puts
    // it in the provisioning profile.
    let app = xcodebuild(
        &ios_project()?,
        &format!("platform=iOS,id={udid}"),
        &["-allowProvisioningUpdates", "-allowProvisioningDeviceRegistration"],
    )?;
    run(Command::new("xcrun").args(["devicectl", "device", "install", "app", "--device", &device]).arg(&app))?;
    println!("the game goes in the app's Documents/game: cargo xtask ios-game, or Finder > device > Files > Glue 5");
    // The first launch right after an install often fails ("Invalid
    // argument"); one a moment later works.
    let started = Instant::now();
    match ios_run(&device) {
        Err(e) if started.elapsed() < Duration::from_secs(10) => {
            eprintln!("launch failed ({e:#}); trying again");
            std::thread::sleep(Duration::from_secs(3));
            ios_run(&device)
        }
        result => result,
    }
}

/// Launches the installed app with its console attached.
fn ios_run(device: &str) -> Result<()> {
    let mut launch = Command::new("xcrun");
    launch.args(["devicectl", "device", "process", "launch", "--console", "--terminate-existing", "--device", device]);
    launch.arg(IOS_BUNDLE_ID);
    // The app gets the same switches as a desktop run.
    for (key, value) in env::vars() {
        if key.starts_with("GLUE5_") && key != "GLUE5_TEAM" && key != "GLUE5_GAME_DIR" {
            launch.env(format!("DEVICECTL_CHILD_{key}"), value);
        }
    }
    run(launch.env("DEVICECTL_CHILD_RUST_LOG", env::var("RUST_LOG").unwrap_or_else(|_| "info".into())))
}

fn ios_game(device: Option<&str>) -> Result<()> {
    let root = workspace_root();
    let game = env::var_os("GLUE5_GAME_DIR").map_or_else(|| root.join("game"), PathBuf::from);
    let game = game.canonicalize().with_context(|| format!("game directory {}", game.display()))?;
    let (device, _) = connected_device(device)?;
    // What the app reads: data/ and each build's translated shaders.
    let mut items = vec![("data".to_string(), game.join("data"))];
    for build in fs::read_dir(game.join("b"))? {
        let shaders = build?.path().join("shaders");
        if shaders.join("index.json").is_file() {
            let name = shaders.strip_prefix(&game)?.to_string_lossy().into_owned();
            items.push((name, shaders));
        }
    }
    for (name, path) in items {
        println!("copying {name} (unchanged files are skipped)");
        run(Command::new("xcrun").args(["devicectl", "device", "copy", "to", "--device", &device]).args([
            "--domain-type",
            "appDataContainer",
            "--domain-identifier",
            IOS_BUNDLE_ID,
            "--source",
            &path.to_string_lossy(),
            "--destination",
            &format!("Documents/game/{name}"),
        ]))?;
    }
    Ok(())
}

/// A connected iOS device (its identifier, name or UDID; default: the only
/// one connected): its CoreDevice identifier and its UDID.
fn connected_device(device: Option<&str>) -> Result<(String, String)> {
    let list = |filter: &str| -> Result<Vec<(String, String)>> {
        let out = Command::new("xcrun")
            .args(["devicectl", "list", "devices", "--hide-default-columns", "--hide-headers"])
            .args(["--columns", "Identifier", "--columns", "udid", "--filter", filter])
            .output()?;
        ensure!(out.status.success(), "devicectl list devices failed");
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut cols = l.split_whitespace();
                Some((cols.next()?.to_string(), cols.next()?.to_string()))
            })
            .filter(|(id, _)| id.len() >= 25)
            .collect())
    };
    let connected = list("State == 'connected'")?;
    let found = match device {
        None => connected,
        Some(d) => {
            let by_id: Vec<_> = connected.iter().filter(|(id, udid)| id == d || udid == d).cloned().collect();
            if by_id.is_empty() { list(&format!("State == 'connected' AND Name == '{d}'"))? } else { by_id }
        }
    };
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no such connected iOS device"),
        _ => bail!(
            "several devices connected: pass one ({})",
            found.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}
