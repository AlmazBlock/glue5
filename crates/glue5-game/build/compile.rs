//! Compiles C files through a content-addressed object cache.
//!
//! The generated game is ~1 GB of C; cc would rebuild all of it whenever the
//! build script reruns. Objects are keyed by (compiler command, source, extra
//! key), so a hook change recompiles only the files it touches, and debug and
//! release builds share the game's objects.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fs, thread};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub fn compile(
    build: &cc::Build,
    sources: &[PathBuf],
    extra_key: &str,
    cache: &Path,
    jobs: usize,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(cache)?;
    let tool = build.get_compiler();
    let base_key = format!("{:?} {:?} {extra_key}", tool.path(), tool.args());

    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let objects = Mutex::new(vec![PathBuf::new(); sources.len()]);
    let error = Mutex::new(None);
    thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= sources.len() || error.lock().unwrap().is_some() {
                        break;
                    }
                    match compile_one(&tool, &base_key, &sources[i], cache) {
                        Ok(obj) => objects.lock().unwrap()[i] = obj,
                        Err(e) => *error.lock().unwrap() = Some(e),
                    }
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!("[{n}/{}] {}", sources.len(), sources[i].display());
                }
            });
        }
    });
    if let Some(e) = error.into_inner().unwrap() {
        return Err(e);
    }
    Ok(objects.into_inner().unwrap())
}

fn compile_one(tool: &cc::Tool, base_key: &str, src: &Path, cache: &Path) -> Result<PathBuf> {
    let text = fs::read(src).with_context(|| format!("reading {}", src.display()))?;
    let mut hash = Sha256::new();
    hash.update(base_key.as_bytes());
    hash.update(src.to_string_lossy().as_bytes());
    hash.update(&text);
    let digest = hash.finalize();
    let name = digest.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>();
    let obj = cache.join(format!("{name}.o"));
    if obj.exists() {
        return Ok(obj);
    }
    let tmp = cache.join(format!("{name}.o.tmp{}", std::process::id()));
    let mut cmd: Command = tool.to_command();
    cmd.arg("-c").arg(src);
    if tool.is_like_msvc() {
        cmd.arg(format!("-Fo{}", tmp.display()));
    } else {
        cmd.arg("-o").arg(&tmp);
    }
    let output = cmd.output().with_context(|| format!("running {cmd:?}"))?;
    if !output.status.success() {
        bail!("compiling {} failed:\n{}", src.display(), String::from_utf8_lossy(&output.stderr));
    }
    fs::rename(&tmp, &obj)?;
    Ok(obj)
}

pub fn archive(build: &cc::Build, objects: &[PathBuf], lib: &Path) -> Result<()> {
    let _ = fs::remove_file(lib);
    let mut ar = build.get_archiver();
    if build.get_compiler().is_like_msvc() {
        // lib.exe or llvm-lib
        ar.arg("-nologo").arg(format!("-out:{}", lib.display())).args(objects);
    } else {
        ar.arg("crs").arg(lib).args(objects);
    }
    let status = ar.status().with_context(|| format!("running {ar:?}"))?;
    if !status.success() {
        bail!("{ar:?} failed");
    }
    Ok(())
}
