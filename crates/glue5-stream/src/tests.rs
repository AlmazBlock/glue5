//! A server and a client on this machine, over a small made-up game.

use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cache::BLOCK;
use crate::client::{Client, Options, Phase};
use crate::protocol::{DATA, HEADER, HELLO, Header};
use crate::server::{self, Server};

const BUILD: &str = "testbuild";

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("glue5-stream-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn game(dir: &Path) -> Vec<u8> {
    let big = noise(3 << 20 | 123, 7);
    let put = |path: &str, data: &[u8]| {
        let p = dir.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    };
    put("data/x64/big.rpf", &big);
    put("data/x64/later.rpf", &noise(1 << 20, 9));
    put("data/common/data/a.xml", &b"<a>hello</a>".repeat(1000));
    put("data/common/data/empty.meta", b"");
    put("data/manifest.json", b"{}");
    put("data/bootset.json", br#"[["x64/big.rpf", [[0, 5000], [1048576, 1100000]]]]"#);
    put(&format!("b/{BUILD}/shaders/index.json"), br#"{"00":{"ok":true}}"#);
    put(&format!("b/{BUILD}/shaders/00.wgsl"), b"fn main() {}");
    put(&format!("b/{BUILD}/shaders/pack0.x.bin"), b"skipped");
    big
}

fn serve(dir: &Path, cache: &Path, addr: SocketAddr) -> Server {
    let opts = server::Options { game: dir.to_owned(), build: BUILD.into(), cache: cache.to_owned(), rate: None };
    let prepared = server::prepare(opts, &|_| {}).unwrap();
    Server::start(&prepared, addr).unwrap()
}

/// Between a client and a server: passes on what each sends, or holds it
/// (the client's requests, the server's answers), or changes the headers
/// of the server's messages.
struct Proxy {
    addr: SocketAddr,
    up: Arc<AtomicBool>,
    down: Arc<AtomicBool>,
    accepted: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<TcpStream>>>,
}

type Tamper = Arc<dyn Fn(&mut Header) + Send + Sync>;

impl Proxy {
    fn start(server: SocketAddr, tamper: Option<Tamper>) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        // Little room for requests the proxy holds: the client's writes stop.
        let _ = socket2::SockRef::from(&listener).set_recv_buffer_size(16 << 10);
        let p = Proxy {
            addr: listener.local_addr().unwrap(),
            up: Arc::new(AtomicBool::new(true)),
            down: Arc::new(AtomicBool::new(true)),
            accepted: Arc::new(AtomicUsize::new(0)),
            done: Arc::new(AtomicBool::new(false)),
            sockets: Arc::new(Mutex::new(Vec::new())),
        };
        let (up, down, accepted, done, sockets) = (p.up.clone(), p.down.clone(), p.accepted.clone(), p.done.clone(), p.sockets.clone());
        std::thread::spawn(move || {
            for client in listener.incoming() {
                if done.load(Ordering::Acquire) {
                    break;
                }
                let (Ok(client), Ok(server)) = (client, TcpStream::connect(server)) else { continue };
                accepted.fetch_add(1, Ordering::AcqRel);
                sockets.lock().unwrap().extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                pump(client.try_clone().unwrap(), server.try_clone().unwrap(), up.clone(), done.clone(), None);
                pump(server, client, down.clone(), done.clone(), tamper.clone());
            }
        });
        p
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.addr);
        for s in self.sockets.lock().unwrap().drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

/// Passes `from`'s bytes on to `to` while `pass` (with `tamper`, a message
/// at a time).
fn pump(mut from: TcpStream, mut to: TcpStream, pass: Arc<AtomicBool>, done: Arc<AtomicBool>, tamper: Option<Tamper>) {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 1 << 20];
        loop {
            while !pass.load(Ordering::Acquire) {
                if done.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let n = match &tamper {
                Some(tamper) => {
                    let Ok(mut h) = Header::read(&mut from) else { break };
                    let n = HEADER + if matches!(h.kind, HELLO | DATA) { h.len as usize } else { 0 };
                    if from.read_exact(&mut buf[HEADER..n]).is_err() {
                        break;
                    }
                    tamper(&mut h);
                    buf[..HEADER].copy_from_slice(&h.encode());
                    n
                }
                None => match from.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                },
            };
            if to.write_all(&buf[..n]).is_err() {
                break;
            }
        }
        let _ = from.shutdown(Shutdown::Both);
        let _ = to.shutdown(Shutdown::Both);
    });
}

/// Whether `f` returns within a second.
fn promptly(f: impl FnOnce() + Send + 'static) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(1)).is_ok()
}

fn wait_ready(c: &Client) {
    let started = Instant::now();
    while !c.is_ready() {
        assert!(started.elapsed() < Duration::from_secs(30), "not ready: {:?}", c.status());
        if let Phase::Failed(e) = c.status().phase {
            panic!("{e}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn end_to_end() {
    let dir = temp("game");
    let big = game(&dir);
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let addr = server.addr();
    let root = temp("copy");
    let opts = || Options { server: addr.to_string(), root: root.clone(), build: BUILD.into(), low_memory: false };
    let client = Client::start(opts());
    wait_ready(&client);

    // The packs brought the small files and the shaders, whole.
    let xml = client.find("data/common/data/a.xml").unwrap();
    assert!(client.path(xml).unwrap().is_file());
    assert_eq!(fs::read(root.join(format!("b/{BUILD}/shaders/00.wgsl"))).unwrap(), b"fn main() {}");
    assert!(!root.join(format!("b/{BUILD}/shaders/pack0.x.bin")).exists());
    let manifest: serde_json::Value = serde_json::from_slice(client.engine_manifest().unwrap()).unwrap();
    assert_eq!(manifest["mount"], "/game/");
    assert!(manifest["files"].as_array().unwrap().iter().all(|f| !f[0].as_str().unwrap().starts_with("b/")));
    assert!(!manifest.to_string().contains("bootset"));

    // Reads from several threads, cached or not, byte for byte.
    let id = client.find("data/x64/big.rpf").unwrap();
    let big = Arc::new(big);
    let threads: Vec<_> = (0..8u64)
        .map(|t| {
            let (client, big) = (client.clone(), big.clone());
            std::thread::spawn(move || {
                for i in 0..40u64 {
                    let off = (t * 7919 + i * 104729 * 13) % big.len() as u64;
                    let len = 1 + ((t + 1) * (i + 3) * 4099) as usize % 200_000;
                    let mut buf = vec![0u8; len];
                    let n = client.read(id, off, &mut buf).unwrap();
                    let end = (off as usize + len).min(big.len());
                    assert_eq!(n, end - off as usize);
                    assert_eq!(&buf[..n], &big[off as usize..end]);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let mut tail = [0u8; 300];
    assert_eq!(client.read(id, big.len() as u64 - 100, &mut tail), Some(100));
    assert_eq!(&tail[..100], &big[big.len() - 100..]);

    // The boot set arrives by itself.
    let started = Instant::now();
    while client.status().boot.0 < client.status().boot.1 {
        assert!(started.elapsed() < Duration::from_secs(10), "boot set: {:?}", client.status().boot);
        std::thread::sleep(Duration::from_millis(20));
    }

    // The server goes away and comes back on the same port.
    client.hint(id, 2 << 20, BLOCK, true);
    drop(server);
    let mut buf = vec![0u8; 4096];
    assert_eq!(client.read(id, 0, &mut buf), Some(4096));
    let back = serve(&dir, &dir.join("packs"), addr);
    let later = client.find("data/x64/later.rpf").unwrap();
    let mut buf = vec![0u8; 8192];
    assert_eq!(client.read(later, 300_000, &mut buf), Some(8192));
    assert_eq!(&buf[..], &noise(1 << 20, 9)[300_000..300_000 + 8192]);
    client.stop();
    drop(back);

    // Without a server, the copy plays.
    let offline = Client::start(Options { server: "127.0.0.1:1".into(), ..opts() });
    wait_ready(&offline);
    let mut buf = vec![0u8; 5000];
    assert_eq!(offline.read(id, 0, &mut buf), Some(5000));
    assert_eq!(&buf[..], &big[..5000]);
    offline.stop();
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn demand_overtakes_prefetch() {
    let dir = temp("prio");
    let data = noise(32 << 20, 3);
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("data/huge.rpf"), &data).unwrap();
    fs::create_dir_all(dir.join(format!("b/{BUILD}/shaders"))).unwrap();
    fs::write(dir.join(format!("b/{BUILD}/shaders/index.json")), b"{}").unwrap();
    // 8 MB/s: the whole file takes 4 s.
    let opts = server::Options { game: dir.clone(), build: BUILD.into(), cache: dir.join("packs"), rate: Some(8_000_000) };
    let prepared = server::prepare(opts, &|_| {}).unwrap();
    let server = Server::start(&prepared, "127.0.0.1:0".parse().unwrap()).unwrap();
    let root = temp("prio-copy");
    let client = Client::start(Options {
        server: server.addr().to_string(),
        root: root.clone(),
        build: BUILD.into(),
        low_memory: false,
    });
    wait_ready(&client);
    let id = client.find("data/huge.rpf").unwrap();
    client.hint(id, 0, data.len() as u64, true);
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    let mut buf = vec![0u8; 4096];
    let off = data.len() as u64 - 4096;
    assert_eq!(client.read(id, off, &mut buf), Some(4096));
    assert_eq!(&buf[..], &data[off as usize..]);
    let waited = started.elapsed();
    assert!(waited < Duration::from_millis(100), "the demand read waited {waited:?} behind the prefetch");
    client.stop();
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn counts_reads_not_announced_in_time() {
    let dir = temp("late");
    let data = noise(16 << 20, 5);
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("data/a.rpf"), &data).unwrap();
    fs::write(dir.join("data/b.rpf"), &data[..1 << 20]).unwrap();
    fs::create_dir_all(dir.join(format!("b/{BUILD}/shaders"))).unwrap();
    fs::write(dir.join(format!("b/{BUILD}/shaders/index.json")), b"{}").unwrap();
    // 8 MB/s: what is announced takes two seconds to come.
    let opts = server::Options { game: dir.clone(), build: BUILD.into(), cache: dir.join("packs"), rate: Some(8_000_000) };
    let prepared = server::prepare(opts, &|_| {}).unwrap();
    let server = Server::start(&prepared, "127.0.0.1:0".parse().unwrap()).unwrap();
    let root = temp("late-copy");
    let client = Client::start(Options {
        server: server.addr().to_string(),
        root: root.clone(),
        build: BUILD.into(),
        low_memory: false,
    });
    wait_ready(&client);
    let (a, b) = (client.find("data/a.rpf").unwrap(), client.find("data/b.rpf").unwrap());
    client.hint(a, 0, data.len() as u64, true);
    std::thread::sleep(Duration::from_millis(300));
    // Announced in time, on its way: waits, but counts not.
    let at = 12 << 20;
    let mut buf = vec![0u8; 256 << 10];
    assert_eq!(client.read(a, at, &mut buf[..4096]), Some(4096));
    // Its first block is here now (with the rest of its request), the
    // others on their way: announced in time too.
    assert_eq!(client.read(a, at + 4000, &mut buf), Some(buf.len()));
    assert_eq!(&buf[..], &data[at as usize + 4000..][..buf.len()]);
    assert_eq!(client.not_in_time(), 0);
    // Never announced.
    assert_eq!(client.read(b, 5000, &mut buf[..100]), Some(100));
    assert_eq!(client.not_in_time(), 1);
    client.stop();
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reads_ahead_only_what_is_played() {
    let dir = temp("ahead");
    let data = noise(4 << 20, 11);
    fs::create_dir_all(dir.join("data")).unwrap();
    fs::write(dir.join("data/music.rpf"), &data).unwrap();
    fs::write(dir.join("data/model.rpf"), &data).unwrap();
    fs::create_dir_all(dir.join(format!("b/{BUILD}/shaders"))).unwrap();
    fs::write(dir.join(format!("b/{BUILD}/shaders/index.json")), b"{}").unwrap();
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let root = temp("ahead-copy");
    let client = Client::start(Options {
        server: server.addr().to_string(),
        root: root.clone(),
        build: BUILD.into(),
        low_memory: false,
    });
    wait_ready(&client);
    let (music, model) = (client.find("data/music.rpf").unwrap(), client.find("data/model.rpf").unwrap());
    // Played: 32 KB every 50 ms. What follows comes before it is read.
    let mut buf = vec![0u8; 32 << 10];
    for k in 0..3 {
        assert_eq!(client.read(music, k * (32 << 10), &mut buf), Some(buf.len()));
        std::thread::sleep(Duration::from_millis(50));
    }
    let started = Instant::now();
    while !client.has(music, 320 << 10) {
        assert!(started.elapsed() < Duration::from_secs(2), "nothing read ahead of the music");
        std::thread::sleep(Duration::from_millis(10));
    }
    // Small pieces of one resource read back to back: nothing past them.
    let mut piece = vec![0u8; 16 << 10];
    for k in 0..3 {
        assert_eq!(client.read(model, k * (16 << 10), &mut piece), Some(piece.len()));
        assert_eq!(&piece[..], &data[(k * (16 << 10)) as usize..][..piece.len()]);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(!client.has(model, 48 << 10), "read ahead of small pieces read back to back");
    client.stop();
    drop(server);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn boot_set_follows_the_engine() {
    let dir = temp("follow");
    let (a, b) = (noise(8 << 20, 11), noise(8 << 20, 13));
    fs::create_dir_all(dir.join("data/x64")).unwrap();
    fs::write(dir.join("data/x64/a.rpf"), &a).unwrap();
    fs::write(dir.join("data/x64/b.rpf"), &b).unwrap();
    fs::write(dir.join("data/bootset.json"), br#"[["x64/a.rpf", [[0, 8388607]]], ["x64/b.rpf", [[0, 8388607]]]]"#).unwrap();
    fs::create_dir_all(dir.join(format!("b/{BUILD}/shaders"))).unwrap();
    fs::write(dir.join(format!("b/{BUILD}/shaders/index.json")), b"{}").unwrap();
    // 8 MB/s: the boot set takes two seconds, a.rpf first.
    let opts = server::Options { game: dir.clone(), build: BUILD.into(), cache: dir.join("packs"), rate: Some(8_000_000) };
    let prepared = server::prepare(opts, &|_| {}).unwrap();
    let server = Server::start(&prepared, "127.0.0.1:0".parse().unwrap()).unwrap();
    let root = temp("follow-copy");
    let client = Client::start(Options {
        server: server.addr().to_string(),
        root: root.clone(),
        build: BUILD.into(),
        low_memory: false,
    });
    wait_ready(&client);
    let (ia, ib) = (client.find("data/x64/a.rpf").unwrap(), client.find("data/x64/b.rpf").unwrap());
    // The engine starts with b.rpf: the rest of it comes before a.rpf's.
    let mut buf = [0u8; 4096];
    assert_eq!(client.read(ib, 0, &mut buf), Some(4096));
    assert_eq!(&buf[..], &b[..4096]);
    let started = Instant::now();
    while !client.has(ib, 6 << 20) {
        assert!(started.elapsed() < Duration::from_secs(10), "b.rpf does not come");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!client.has(ia, 7 << 20), "a.rpf came first");
    client.stop();
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn downloads_the_whole_game() {
    let dir = temp("whole");
    game(&dir);
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let root = temp("whole-copy");
    let client = Client::start(Options {
        server: server.addr().to_string(),
        root: root.clone(),
        build: BUILD.into(),
        low_memory: false,
    });
    wait_ready(&client);
    assert!(root.join(crate::cache::MARKER).exists() && !client.status().complete);
    client.set_fill(true);
    let started = Instant::now();
    while !client.status().complete {
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", client.status());
        std::thread::sleep(Duration::from_millis(20));
    }
    // A game folder like any other: every file whole, no marker.
    assert!(!root.join(crate::cache::MARKER).exists());
    for f in ["data/x64/big.rpf", "data/x64/later.rpf", "data/common/data/a.xml", "data/common/data/empty.meta"] {
        assert_eq!(fs::read(root.join(f)).unwrap(), fs::read(dir.join(f)).unwrap(), "{f}");
    }
    client.stop();
    drop(server);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

/// The whole game is complete once the copy says so on disk (no marker):
/// until then, a partial copy.
#[cfg(unix)]
#[test]
fn completes_only_once_finished() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp("unfinished");
    game(&dir);
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let root = temp("unfinished-copy");
    let client = Client::start(Options {
        server: server.addr().to_string(),
        root: root.clone(),
        build: BUILD.into(),
        low_memory: false,
    });
    wait_ready(&client);
    // The files can be written, nothing beside them (the index, the marker).
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
    client.set_fill(true);
    let started = Instant::now();
    while client.status().cache_bytes < client.status().total_bytes {
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", client.status());
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(!client.status().complete && root.join(crate::cache::MARKER).exists());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    while !client.status().complete {
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", client.status());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!root.join(crate::cache::MARKER).exists());
    client.stop();
    drop(server);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

/// A server that takes no more requests holds up neither the app's status
/// (asked every frame) nor stopping the client.
#[test]
fn a_stuck_server_holds_nothing_up() {
    let dir = temp("stuck");
    fs::create_dir_all(dir.join("data")).unwrap();
    // 8 GB, sparse: 4 MB of requests.
    fs::File::create(dir.join("data/huge.rpf")).unwrap().set_len(8 << 30).unwrap();
    fs::create_dir_all(dir.join(format!("b/{BUILD}/shaders"))).unwrap();
    fs::write(dir.join(format!("b/{BUILD}/shaders/index.json")), b"{}").unwrap();
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let proxy = Proxy::start(server.addr(), None);
    let root = temp("stuck-copy");
    let client = Client::start(Options { server: proxy.addr.to_string(), root: root.clone(), build: BUILD.into(), low_memory: false });
    wait_ready(&client);
    let id = client.find("data/huge.rpf").unwrap();
    proxy.up.store(false, Ordering::Release);
    let c = client.clone();
    std::thread::spawn(move || {
        for k in 0..64 {
            c.hint(id, k << 27, 1 << 27, true);
        }
    });
    std::thread::sleep(Duration::from_secs(1));
    let c = client.clone();
    assert!(promptly(move || assert!(c.status().connected)), "status() waits for a stuck write");
    let c = client.clone();
    assert!(promptly(move || c.stop()), "stop() waits for a stuck write");
    drop(proxy);
    drop(server);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn checks_a_server_before_it_is_chosen() {
    let dir = temp("check");
    game(&dir);
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let addr = server.addr().to_string();
    assert!(matches!(crate::check(&addr, BUILD), crate::Check::Ready { .. }));
    assert!(matches!(crate::check(&addr, "otherbuild"), crate::Check::Unusable(why) if why.contains("otherbuild")));
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
    assert!(matches!(crate::check(&closed, BUILD), crate::Check::Unreachable(_)));
}

#[test]
fn prepares_side_by_side() {
    let dir = temp("prepare");
    game(&dir);
    let cache = dir.join("packs");
    // The app sharing again before its first preparation was done.
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let opts = server::Options { game: dir.clone(), build: BUILD.into(), cache: cache.clone(), rate: None };
            std::thread::spawn(move || server::prepare(opts, &|_| {}).map(|_| ()))
        })
        .collect();
    for j in jobs {
        j.join().unwrap().unwrap();
    }
    let mut names: Vec<String> = fs::read_dir(&cache).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    for name in names {
        assert!(name.ends_with(".zst"), "{name}");
        crate::pack::unpack(fs::File::open(cache.join(&name)).unwrap(), |_, _| Ok(())).unwrap();
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn leaves_the_files_of_a_build_going_on() {
    let dir = temp("leftovers");
    game(&dir);
    let cache = dir.join("packs");
    fs::create_dir_all(&cache).unwrap();
    let put = |name: &str, age: Duration| {
        let f = fs::File::create(cache.join(name)).unwrap();
        f.set_modified(std::time::SystemTime::now() - age).unwrap();
    };
    put("text-0123456789abcdef.4242-0.tmp", Duration::ZERO);
    put("shaders-0123456789abcdef.4242-1.tmp", Duration::from_secs(7200));
    put("text-fedcba9876543210.zst", Duration::from_secs(60));
    let opts = server::Options { game: dir.clone(), build: BUILD.into(), cache: cache.clone(), rate: None };
    server::prepare(opts, &|_| {}).unwrap();
    // Another server's build, going on: left alone; one that died, and an
    // older pack: gone.
    assert!(cache.join("text-0123456789abcdef.4242-0.tmp").exists());
    assert!(!cache.join("shaders-0123456789abcdef.4242-1.tmp").exists());
    assert!(!cache.join("text-fedcba9876543210.zst").exists());
    let _ = fs::remove_dir_all(&dir);
}

/// A request past a file's end (even past what 64 bits hold) gets what
/// there is of it, and its end.
#[test]
fn answers_a_read_past_the_end() {
    use crate::protocol::*;
    use std::io::Write;
    let dir = temp("past");
    game(&dir);
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let mut s = std::net::TcpStream::connect(server.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let hello = Header::read(&mut s).unwrap();
    payload(&mut s, hello.len, MAX_MESSAGE).unwrap();
    let read = Header { kind: READ, prio: DEMAND, req: 1, off: u64::MAX - 100, len: 4096, file: 0, crc: 0 };
    s.write_all(&read.encode()).unwrap();
    let end = Header::read(&mut s).unwrap();
    assert_eq!((end.kind, end.req, end.len), (END, 1, SHORT));
    drop(s);
    let started = Instant::now();
    while server.connections() > 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "the connection stays open");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(server);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn keeps_no_connection_its_client_closed() {
    let dir = temp("closed");
    game(&dir);
    let server = serve(&dir, &dir.join("packs"), "127.0.0.1:0".parse().unwrap());
    let addr = server.addr().to_string();
    // A phone's Servers page checks every few seconds.
    for _ in 0..20 {
        assert!(matches!(crate::check(&addr, BUILD), crate::Check::Ready { .. }));
    }
    let started = Instant::now();
    while server.connections() > 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "{} connections still open", server.connections());
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(server);
    let _ = fs::remove_dir_all(&dir);
}
