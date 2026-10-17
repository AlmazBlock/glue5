//! A server and a client on this machine, over a small made-up game.

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cache::BLOCK;
use crate::client::{Client, Options, Phase};
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
