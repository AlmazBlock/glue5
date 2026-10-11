//! The engine's own output: its log (tens of thousands of lines while loading)
//! and whatever it prints to stdout/stderr.
//!
//! Engine threads only queue it; a background thread writes it to
//! `<user dir>/game.log`, and to the terminal too with `GLUE5_ENGINE_LOG=stdout`.
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

fn run(rx: Receiver<Vec<u8>>, file: File, to_stdout: bool) {
    crate::qos::set(crate::qos::Qos::Utility);
    let mut file = BufWriter::with_capacity(1 << 20, file);
    let mut stdout = to_stdout.then(io::stdout);
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(bytes) => {
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
