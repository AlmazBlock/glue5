//! The engine's own output: its log (tens of thousands of lines while loading)
//! and whatever it prints to stdout/stderr.
//!
//! Engine threads only queue it; a background thread writes it to
//! `<user dir>/game.log`, and to the terminal too with `GLUE5_ENGINE_LOG=stdout`
//! (on Android, where stdout goes nowhere, to the log).
//! Printing synchronously from engine threads stalled the engine behind a
//! slow terminal: loading looked stuck.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::Duration;

/// Queued writes before new ones are dropped (the writer only falls this far
/// behind on a terminal that cannot keep up).
const QUEUE: usize = 1 << 16;

static SINK: OnceLock<SyncSender<Vec<u8>>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Starts the writer; returns the log file's path.
pub(crate) fn start(user_dir: &Path) -> io::Result<PathBuf> {
    let path = user_dir.join("game.log");
    let file = File::create(&path)?;
    let to_stdout = std::env::var("GLUE5_ENGINE_LOG").is_ok_and(|v| v == "stdout");
    let (tx, rx) = sync_channel(QUEUE);
    if SINK.set(tx).is_ok() {
        thread::Builder::new().name("engine log".into()).spawn(move || run(rx, file, to_stdout))?;
    }
    Ok(path)
}

/// Queues engine output. Never blocks.
pub(crate) fn write(bytes: &[u8]) {
    let Some(sink) = SINK.get() else {
        let _ = io::stdout().write_all(bytes);
        return;
    };
    if let Err(TrySendError::Full(_)) = sink.try_send(bytes.to_vec()) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Queues one line of engine output.
pub(crate) fn line(text: &str) {
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(b'\n');
    write(&bytes);
}

/// A fatal error the engine reports ("Fatal Error: ..."): it then waits for
/// an OK that never comes ("Press Ok to abort the program"), the screen
/// frozen, so the game has stopped.
fn fatal_error(bytes: &[u8]) -> Option<String> {
    const MARK: &[u8] = b"Fatal Error: ";
    let at = bytes.windows(MARK.len()).position(|w| w == MARK)? + MARK.len();
    let rest = &bytes[at..];
    let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
    Some(format!("the engine failed: {}", String::from_utf8_lossy(&rest[..end]).trim()))
}

fn run(rx: Receiver<Vec<u8>>, file: File, to_stdout: bool) {
    crate::qos::set(crate::qos::Qos::Utility);
    let mut file = BufWriter::with_capacity(1 << 20, file);
    #[cfg(not(target_os = "android"))]
    let mut stdout = to_stdout.then(io::stdout);
    #[cfg(target_os = "android")]
    let mut stdout = to_stdout.then(Logcat::default);
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(bytes) => {
                if let Some(why) = fatal_error(&bytes) {
                    crate::exit::fail(why);
                }
                let _ = file.write_all(&bytes);
                if let Some(out) = &mut stdout {
                    let _ = out.write_all(&bytes);
                }
            }
            // Idle: make the file current (for `tail -f`).
            Err(RecvTimeoutError::Timeout) => {
                let dropped = DROPPED.swap(0, Ordering::Relaxed);
                if dropped > 0 {
                    let _ =
                        writeln!(file, "[glue5] {dropped} engine output writes dropped (the log could not keep up)");
                }
                let _ = file.flush();
                if let Some(out) = &mut stdout {
                    let _ = out.flush();
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = file.flush();
}

/// Engine output to the log, line by line.
#[cfg(target_os = "android")]
#[derive(Default)]
struct Logcat(Vec<u8>);

#[cfg(target_os = "android")]
impl Write for Logcat {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(bytes);
        while let Some(end) = self.0.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.0.drain(..=end).collect();
            log::info!(target: "engine", "{}", String::from_utf8_lossy(&line).trim_end());
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_engine_s_fatal_errors() {
        let line = b"Fatal Error: <HDD Reader> Short read on /game/x.rpf - got -1\n";
        assert_eq!(
            fatal_error(line).as_deref(),
            Some("the engine failed: <HDD Reader> Short read on /game/x.rpf - got -1")
        );
        assert_eq!(fatal_error(b"Error: something else\n"), None);
    }
}
