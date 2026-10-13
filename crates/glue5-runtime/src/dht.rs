//! Finding the other players of a lobby on the Internet, with no server of
//! ours: the BitTorrent DHT (BEP 5), millions of public nodes that keep, for
//! a 20-byte key, the addresses announced under it. A lobby is a key; each
//! player announces itself under it and asks for the others.
//!
//! The DHT runs over the game's own UDP socket, and announces with the
//! port the nodes see (`implied_port`): the address it hands out is the
//! one the player's NAT opened for that socket, which the others can send
//! to.
//!
//! The client stays small: it never answers queries (it says so, BEP 43's
//! read-only flag) and keeps no routing table, only the nodes of one lookup
//! (the nodes closest to the lobby's key, which hold its announces).

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Well-known entry points to the DHT.
const BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
    "dht.libtorrent.org:25401",
];
/// Queries in flight at most, and how long one may take.
const PARALLEL: usize = 8;
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
/// The nodes a lookup ends on (and announces to).
const CLOSEST: usize = 8;
/// Nodes kept as candidates.
const CANDIDATES: usize = 64;
/// How often the closest nodes are asked again for new players (and told
/// about this one): announces expire after about 30 minutes.
const REFRESH: Duration = Duration::from_secs(20);
const ANNOUNCE: Duration = Duration::from_secs(120);
/// A lookup starts over after this (the closest nodes change), or sooner
/// when no node answered.
const RESTART: Duration = Duration::from_secs(600);
const RETRY: Duration = Duration::from_secs(15);

pub type Id = [u8; 20];

struct Node {
    addr: SocketAddrV4,
    queried: bool,
    responded: bool,
    token: Option<Vec<u8>>,
}

pub struct Dht {
    id: Id,
    key: Id,
    port: u16,
    /// By distance to the key.
    nodes: BTreeMap<Id, Node>,
    /// Queries in flight: transaction id -> node (by distance), sent at.
    pending: HashMap<u16, (Id, Instant)>,
    next_tid: u16,
    started: Option<Instant>,
    refreshed: Option<Instant>,
    announced: Option<Instant>,
}

/// What the socket should send, and the players found.
#[derive(Default)]
pub struct Output {
    pub send: Vec<(SocketAddrV4, Vec<u8>)>,
    pub peers: Vec<SocketAddrV4>,
}

fn distance(a: &Id, b: &Id) -> Id {
    std::array::from_fn(|i| a[i] ^ b[i])
}

impl Dht {
    /// A client looking for `key`'s players, announcing `port`.
    pub fn new(id: Id, key: Id, port: u16) -> Dht {
        Dht {
            id,
            key,
            port,
            nodes: BTreeMap::new(),
            pending: HashMap::new(),
            next_tid: 0,
            started: None,
            refreshed: None,
            announced: None,
        }
    }

    /// Whether a datagram is the DHT's (a bencoded dictionary).
    pub fn is_dht(data: &[u8]) -> bool {
        data.first() == Some(&b'd')
    }

    /// Advances the lookup; call it a few times a second.
    pub fn tick(&mut self, now: Instant, out: &mut Output) {
        self.pending.retain(|_, (_, at)| now - *at < QUERY_TIMEOUT);
        let restart = self.started.is_none_or(|s| {
            let age = now - s;
            age > RESTART || (age > RETRY && !self.nodes.values().any(|n| n.responded))
        });
        if restart {
            self.start(now, out);
            return;
        }
        // The lookup: the closest nodes not asked yet.
        let busy = self.pending.len();
        let ask: Vec<Id> = self
            .nodes
            .iter()
            .take(CANDIDATES)
            .filter(|(_, n)| !n.queried)
            .take(PARALLEL.saturating_sub(busy))
            .map(|(d, _)| *d)
            .collect();
        for d in ask {
            self.get_peers(d, now, out);
        }
        // Done when the closest nodes that answered were all asked: then
        // ask them again now and then, and announce.
        let settled = self.pending.is_empty() && self.nodes.values().take(CLOSEST).all(|n| n.queried);
        if !settled {
            return;
        }
        if self.refreshed.is_none_or(|r| now - r > REFRESH) {
            self.refreshed = Some(now);
            let closest: Vec<Id> = self.closest().map(|(d, _)| *d).collect();
            for d in closest {
                self.get_peers(d, now, out);
            }
        }
        if self.announced.is_none_or(|a| now - a > ANNOUNCE) && self.closest().any(|(_, n)| n.token.is_some()) {
            self.announced = Some(now);
            let to: Vec<(SocketAddrV4, Vec<u8>)> =
                self.closest().filter_map(|(_, n)| Some((n.addr, n.token.clone()?))).collect();
            for (addr, token) in to {
                let tid = self.tid();
                let query = dict([
                    ("t", B(tid.to_be_bytes().to_vec())),
                    ("y", B(b"q".to_vec())),
                    ("q", B(b"announce_peer".to_vec())),
                    (
                        "a",
                        dict([
                            ("id", B(self.id.to_vec())),
                            ("implied_port", I(1)),
                            ("info_hash", B(self.key.to_vec())),
                            ("port", I(self.port.into())),
                            ("token", B(token)),
                        ]),
                    ),
                    ("ro", I(1)),
                ]);
                out.send.push((addr, query.encode()));
            }
        }
    }

    /// The closest nodes that answered.
    fn closest(&self) -> impl Iterator<Item = (&Id, &Node)> {
        self.nodes.iter().filter(|(_, n)| n.responded).take(CLOSEST)
    }

    fn start(&mut self, now: Instant, out: &mut Output) {
        self.started = Some(now);
        self.refreshed = None;
        self.nodes.clear();
        self.pending.clear();
        // The bootstrap nodes' ids are unknown: they go in as the farthest.
        for (i, addr) in bootstrap().into_iter().enumerate() {
            let mut d = [0xff; 20];
            d[19] = i as u8;
            self.nodes.insert(d, Node { addr, queried: false, responded: false, token: None });
        }
        self.tick(now, out);
    }

    fn tid(&mut self) -> u16 {
        self.next_tid = self.next_tid.wrapping_add(1);
        self.next_tid
    }

    fn get_peers(&mut self, d: Id, now: Instant, out: &mut Output) {
        let tid = self.tid();
        let Some(node) = self.nodes.get_mut(&d) else { return };
        node.queried = true;
        let query = dict([
            ("t", B(tid.to_be_bytes().to_vec())),
            ("y", B(b"q".to_vec())),
            ("q", B(b"get_peers".to_vec())),
            ("a", dict([("id", B(self.id.to_vec())), ("info_hash", B(self.key.to_vec()))])),
            ("ro", I(1)),
        ]);
        out.send.push((node.addr, query.encode()));
        self.pending.insert(tid, (d, now));
    }

    /// A datagram from a DHT node.
    pub fn receive(&mut self, from: SocketAddrV4, data: &[u8], out: &mut Output) {
        let Some(Value::Dict(msg)) = Value::decode(data) else { return };
        let Some(Value::Bytes(t)) = msg.get(&b"t"[..]) else { return };
        let Ok(t) = <[u8; 2]>::try_from(t.as_slice()) else { return };
        let Some((d, _)) = self.pending.remove(&u16::from_be_bytes(t)) else { return };
        let Some(Value::Dict(r)) = msg.get(&b"r"[..]) else { return };
        // The node, under its real id now.
        let Some(mut node) = self.nodes.remove(&d) else { return };
        if node.addr != from {
            self.nodes.insert(d, node);
            return;
        }
        node.responded = true;
        if let Some(Value::Bytes(token)) = r.get(&b"token"[..]) {
            node.token = Some(token.clone());
        }
        let key = match r.get(&b"id"[..]) {
            Some(Value::Bytes(id)) if id.len() == 20 => distance(&self.key, id.as_slice().try_into().unwrap()),
            _ => d,
        };
        self.nodes.insert(key, node);
        if let Some(Value::List(values)) = r.get(&b"values"[..]) {
            for v in values {
                if let Value::Bytes(v) = v
                    && let Some(addr) = compact_addr(v)
                {
                    out.peers.push(addr);
                }
            }
        }
        if let Some(Value::Bytes(nodes)) = r.get(&b"nodes"[..]) {
            for n in nodes.as_chunks::<26>().0 {
                let (Ok(id), Some(addr)) = (<Id>::try_from(&n[..20]), compact_addr(&n[20..])) else { continue };
                let d = distance(&self.key, &id);
                if !self.nodes.contains_key(&d) && self.nodes.values().all(|n| n.addr != addr) {
                    self.nodes.insert(d, Node { addr, queried: false, responded: false, token: None });
                }
            }
            // Only the closest candidates matter.
            while self.nodes.len() > CANDIDATES {
                let last = *self.nodes.keys().next_back().unwrap();
                self.nodes.remove(&last);
            }
        }
    }
}

/// The bootstrap nodes' addresses, looked up away from the network thread
/// (empty until then, or while there is no network).
fn bootstrap() -> Vec<SocketAddrV4> {
    use std::sync::Mutex;
    static RESOLVED: Mutex<(Vec<SocketAddrV4>, bool)> = Mutex::new((Vec::new(), false));
    let mut resolved = RESOLVED.lock().unwrap();
    if resolved.0.is_empty() && !resolved.1 {
        resolved.1 = true;
        std::thread::spawn(|| {
            let addrs: Vec<SocketAddrV4> = BOOTSTRAP
                .iter()
                .filter_map(|host| host.to_socket_addrs().ok())
                .flatten()
                .filter_map(|a| match a {
                    SocketAddr::V4(a) => Some(a),
                    SocketAddr::V6(_) => None,
                })
                .collect();
            log::debug!("dht: {} bootstrap nodes", addrs.len());
            *RESOLVED.lock().unwrap() = (addrs, false);
        });
    }
    resolved.0.clone()
}

fn compact_addr(b: &[u8]) -> Option<SocketAddrV4> {
    let b: [u8; 6] = b.try_into().ok()?;
    let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
    let port = u16::from_be_bytes([b[4], b[5]]);
    (port != 0 && !ip.is_unspecified() && !ip.is_broadcast()).then_some(SocketAddrV4::new(ip, port))
}

// ---- bencode -------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}
use Value::{Bytes as B, Int as I};

fn dict<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Dict(entries.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())
}

impl Value {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Int(i) => out.extend_from_slice(format!("i{i}e").as_bytes()),
            Value::Bytes(b) => {
                out.extend_from_slice(format!("{}:", b.len()).as_bytes());
                out.extend_from_slice(b);
            }
            Value::List(l) => {
                out.push(b'l');
                l.iter().for_each(|v| v.write(out));
                out.push(b'e');
            }
            Value::Dict(d) => {
                out.push(b'd');
                for (k, v) in d {
                    Value::Bytes(k.clone()).write(out);
                    v.write(out);
                }
                out.push(b'e');
            }
        }
    }

    fn decode(data: &[u8]) -> Option<Value> {
        let mut at = 0;
        let v = Value::read(data, &mut at, 0)?;
        Some(v)
    }

    fn read(d: &[u8], at: &mut usize, depth: u32) -> Option<Value> {
        if depth > 8 {
            return None;
        }
        match *d.get(*at)? {
            b'i' => {
                let end = *at + d[*at..].iter().position(|&c| c == b'e')?;
                let i = std::str::from_utf8(&d[*at + 1..end]).ok()?.parse().ok()?;
                *at = end + 1;
                Some(Value::Int(i))
            }
            b'l' => {
                *at += 1;
                let mut l = Vec::new();
                while *d.get(*at)? != b'e' {
                    l.push(Value::read(d, at, depth + 1)?);
                }
                *at += 1;
                Some(Value::List(l))
            }
            b'd' => {
                *at += 1;
                let mut m = BTreeMap::new();
                while *d.get(*at)? != b'e' {
                    let Value::Bytes(k) = Value::read(d, at, depth + 1)? else { return None };
                    m.insert(k, Value::read(d, at, depth + 1)?);
                }
                *at += 1;
                Some(Value::Dict(m))
            }
            b'0'..=b'9' => {
                let colon = *at + d[*at..].iter().position(|&c| c == b':')?;
                let len: usize = std::str::from_utf8(&d[*at..colon]).ok()?.parse().ok()?;
                let b = d.get(colon + 1..colon + 1 + len)?.to_vec();
                *at = colon + 1 + len;
                Some(Value::Bytes(b))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bencode_round_trip() {
        let v = dict([("a", I(-3)), ("b", B(b"xyz".to_vec())), ("c", Value::List(vec![I(1), B(vec![])]))]);
        let e = v.encode();
        assert_eq!(e, b"d1:ai-3e1:b3:xyz1:cli1e0:ee");
        assert_eq!(Value::decode(&e), Some(v));
        assert_eq!(Value::decode(b"d1:a"), None);
    }
}
