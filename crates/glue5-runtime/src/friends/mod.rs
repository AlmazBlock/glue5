//! Friends, with no server of ours: players who agreed to be, who see when
//! the other plays, join each other's games and invite each other, over a
//! channel only the two of them can read.
//!
//! - Each player has a key pair, made on the device (crypto.rs), and a
//!   friend code to give out. Whoever has the code can find its owner and
//!   ask to be friends; the owner says yes or no. Nothing else is shared
//!   with a stranger.
//! - Friends find each other on the BitTorrent DHT and the local network
//!   under keys only the two of them can compute, which change every 6
//!   hours, then open a Noise channel (session.rs) authenticated by their
//!   keys: a player answers no one else, and nothing it says to a friend
//!   is readable or forgeable by anyone else.
//! - Over it they tell each other their name, what they play and, playing
//!   GTA Online, their lobby and session (which a friend joins with); and
//!   they invite each other: to a session (the friend's game asks it
//!   whether to join), and with the game's own invites to jobs (game.rs).
//!
//! What a friend learns is the player's address on the Internet, as any
//! player of a lobby does. The DHT's nodes see two addresses meeting under
//! a key, not who they are.
//!
//! The service is a state machine fed with datagrams and time
//! (`Service`); `start`, `tick`, `receive` and `found` run it on the
//! player's socket (p2p.rs), the application reads and changes it through
//! the functions at the end.

mod crypto;
pub(crate) mod game;
mod session;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crypto::{Identity, Key, Tag};
use session::{Handshake, Pattern, Transport};

use crate::dht;

/// The file of friends, the player's key and code, in the user directory.
const FILE: &str = "friends.json";
/// Friends at most, requests waiting at most.
const MAX_FRIENDS: usize = 200;
const MAX_REQUESTS: usize = 8;
/// A channel stays open while the other side is heard; this much silence
/// and the friend is offline.
const SILENCE: Duration = Duration::from_secs(25);
/// How often something is said on a quiet channel, and the presence again.
const PING: Duration = Duration::from_secs(5);
const PRESENCE: Duration = Duration::from_secs(15);
/// A channel is replaced by a new handshake after this (fresh keys).
const REKEY: Duration = Duration::from_secs(15 * 60);
/// A handshake not answered by then is given up.
const HANDSHAKE: Duration = Duration::from_secs(5);
/// One address is tried for one friend at most this often.
const CONTACT: Duration = Duration::from_secs(2);
/// How often a message waiting for its acknowledgment goes again, and how
/// many times.
const RESEND: Duration = Duration::from_secs(1);
const TRIES: u32 = 10;
/// How often a friend request goes again while unanswered.
const ASK_AGAIN: Duration = Duration::from_secs(3);
/// How often the local network is asked for friends not connected.
const PROBE_EVERY: Duration = Duration::from_secs(5);
const SWEEP_EVERY: Duration = Duration::from_secs(15);
/// Handshakes answered per address, and in all, per this time.
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_PER_ADDRESS: u32 = 10;
const RATE_ALL: u32 = 60;
/// DHT lookups: friends not connected are asked for often.
const LOOKUP_ALONE: Duration = Duration::from_secs(30);
const LOOKUP_CONNECTED: Duration = Duration::from_secs(300);
/// An invite from a friend is shown this long, and one more from it is not
/// heard before then.
const INVITED: Duration = Duration::from_secs(300);
const INVITE_AGAIN: Duration = Duration::from_secs(20);
/// Texts from a friend, at most.
const MAX_NAME: usize = 32;
const MAX_LOBBIES: usize = 4;
const MAX_TEXT: usize = 900;

/// What a player is doing, as its friends see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Playing {
    /// The application is open, no game yet.
    #[default]
    Menu,
    Story,
    Online,
}

/// What a player tells its friends about itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Presence {
    pub name: String,
    /// Its gamer id in GTA Online.
    pub id: u64,
    pub playing: Playing,
    /// The lobbies it plays in, which a friend can join ("" is everyone's).
    #[serde(default)]
    pub lobbies: Vec<String>,
    /// The game's session, as the engine writes its info (game.rs).
    #[serde(default)]
    pub session: Option<String>,
}

impl Presence {
    /// Within bounds, whatever a friend sent.
    fn bounded(mut self) -> Presence {
        self.name = clean_name(&self.name);
        self.lobbies.truncate(MAX_LOBBIES);
        self.lobbies.retain(|l| l.len() <= MAX_NAME);
        if self.session.as_ref().is_some_and(|s| s.len() > MAX_TEXT) {
            self.session = None;
        }
        self
    }
}

/// A name as shown: printable, 32 characters at most.
fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect::<String>().trim().to_owned()
}

/// What friends say to each other on the channel.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
enum Message {
    Presence(Presence),
    /// On a friend code's channel: "let us be friends".
    Request { name: String, id: u64 },
    Accept { name: String, id: u64 },
    Decline,
    /// "We are friends no more."
    Removed,
    /// The game's event for this friend (an invite to a job, an answer to
    /// one), its sequence number.
    Game { seq: u32, text: String },
    /// "Join me": an invite to the sender's session in GTA Online (the
    /// platform's invites, which Social Club's overlay showed).
    Invite { seq: u32 },
    Ack { seq: u32 },
    Ping,
}

// ---- the file --------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct Store {
    /// The private key (hex).
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    friends: Vec<StoredFriend>,
    /// Keys whose requests were declined (hex): not heard again.
    #[serde(default)]
    blocked: Vec<String>,
    /// Friend codes this player asked with, not answered yet.
    #[serde(default)]
    asked: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredFriend {
    key: String,
    name: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    since: u64,
}

// ---- the service -------------------------------------------------------------

fn unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

struct Friend {
    key: Key,
    /// The secret the two share, which their meetings come from.
    shared: Key,
    name: String,
    id: u64,
    since: u64,
    transport: Option<Transport>,
    /// The channel before the last handshake, for datagrams still on the way.
    previous: Option<Transport>,
    presence: Option<Presence>,
    /// When it was last told the presence, and what.
    presence_sent: Option<(Instant, Presence)>,
    /// The newest handshake time from it (ms): an older one is a replay.
    last_time: u64,
    /// Messages waiting for their acknowledgment: seq, payload, sent, tries.
    outbox: Vec<(u32, Vec<u8>, Instant, u32)>,
    next_seq: u32,
    /// Game messages from it already passed on (repeated ones are not).
    seen: VecDeque<u32>,
    /// When it last invited this player to its session.
    invited: Option<Instant>,
    /// Addresses tried for it, when.
    contacted: HashMap<SocketAddrV4, Instant>,
}

impl Friend {
    fn connected(&self) -> bool {
        self.transport.is_some()
    }

    /// Sends a message that is acknowledged (sent again until it is):
    /// false if the friend is not connected.
    fn send_reliably(&mut self, message: impl FnOnce(u32) -> Message, now: Instant, out: &mut Out) -> bool {
        let Some(t) = self.transport.as_mut() else { return false };
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        let Ok(d) = serde_json::to_vec(&message(seq)) else { return false };
        out.push(t.addr, t.seal(&d, now));
        self.outbox.push((seq, d, now, 0));
        true
    }

    /// Acknowledges message `seq`: whether it is the first time it came.
    fn first_time(&mut self, seq: u32, out: &mut Out) -> bool {
        if let Some(t) = self.transport.as_mut() {
            out.push(t.addr, seal(t, &Message::Ack { seq }));
        }
        if self.seen.contains(&seq) {
            return false;
        }
        self.seen.push_back(seq);
        if self.seen.len() > 64 {
            self.seen.pop_front();
        }
        true
    }
}

/// A request to this player, waiting for its answer.
struct Request {
    name: String,
    id: u64,
    transport: Transport,
    /// Told (the Request message came).
    asked: bool,
}

/// A request this player made with a friend's code.
struct Ask {
    code: String,
    transport: Option<Transport>,
    asked: Option<Instant>,
    declined: bool,
    contacted: HashMap<SocketAddrV4, Instant>,
}

/// Who a channel or handshake is with.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Owner {
    Friend(Key),
    /// Someone asking with this player's code (its key once known).
    Asker(Option<Key>),
    /// The owner of a code this player asks with.
    Code(String),
}

/// An event for the game: a friend's invite and the like.
#[derive(Clone, Debug)]
pub(crate) struct GameEvent {
    pub from: u64,
    pub name: String,
    pub text: String,
}

pub(crate) struct Service {
    dir: PathBuf,
    identity: Option<Identity>,
    code: Option<String>,
    /// This player, as told to friends (the lobbies are added as sent).
    local: Presence,
    friends: HashMap<Key, Friend>,
    blocked: HashSet<Key>,
    requests: HashMap<Key, Request>,
    asked: Vec<Ask>,
    handshakes: HashMap<u32, (Handshake, Owner)>,
    /// Open channels by this side's index.
    channels: HashMap<u32, Owner>,
    /// Handshakes answered recently: per address, in all.
    answered: HashMap<Ipv4Addr, (Instant, u32)>,
    answered_all: (Instant, u32),
    /// A random id in this player's probes: its own come back.
    probe_id: [u8; 8],
    own: HashSet<SocketAddrV4>,
    probed: Option<Instant>,
    swept: Option<Instant>,
    /// The game's events from friends, for the game thread.
    events: VecDeque<GameEvent>,
    /// A friend to join in its session (game.rs), since when.
    follow: Option<(Key, Instant)>,
    /// Friends who invited this player to their sessions, for the game
    /// (gamer ids).
    invites: VecDeque<u64>,
    /// Bumped at every change the application shows.
    generation: u64,
}

/// What the service wants sent.
#[derive(Default)]
pub(crate) struct Out {
    pub send: Vec<(SocketAddrV4, Vec<u8>)>,
}

impl Out {
    fn push(&mut self, to: SocketAddrV4, d: Option<Vec<u8>>) {
        if let Some(d) = d {
            self.send.push((to, d));
        }
    }
}

impl Service {
    fn load(dir: &Path, name: &str, id: u64) -> Service {
        let store: Store = std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| match serde_json::from_slice(&b) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::warn!("friends: {FILE}: {e}");
                    None
                }
            })
            .unwrap_or_default();
        let identity = store.key.as_deref().and_then(crypto::unhex::<32>).map(Identity::from_private);
        let mut service = Service {
            dir: dir.to_owned(),
            identity,
            code: store.code.filter(|c| crypto::parse_code(c).is_some()),
            local: Presence { name: clean_name(name), id, ..Presence::default() },
            friends: HashMap::new(),
            blocked: store.blocked.iter().filter_map(|k| crypto::unhex(k)).collect(),
            requests: HashMap::new(),
            asked: Vec::new(),
            handshakes: HashMap::new(),
            channels: HashMap::new(),
            answered: HashMap::new(),
            answered_all: (Instant::now(), 0),
            probe_id: crypto::random(),
            own: HashSet::new(),
            probed: None,
            swept: None,
            events: VecDeque::new(),
            follow: None,
            invites: VecDeque::new(),
            generation: 1,
        };
        for f in store.friends {
            if let Some(key) = crypto::unhex(&f.key) {
                service.add_friend(key, &f.name, f.id, f.since);
            }
        }
        for code in store.asked {
            if let Some(code) = crypto::parse_code(&code) {
                service.asked.push(Ask { code, transport: None, asked: None, declined: false, contacted: HashMap::new() });
            }
        }
        service
    }

    fn save(&self) {
        let store = Store {
            key: self.identity.as_ref().map(|i| crypto::hex(i.private())),
            code: self.code.clone(),
            friends: self
                .friends
                .values()
                .map(|f| StoredFriend { key: crypto::hex(&f.key), name: f.name.clone(), id: f.id, since: f.since })
                .collect(),
            blocked: self.blocked.iter().map(|k| crypto::hex(k)).collect(),
            asked: self.asked.iter().filter(|a| !a.declined).map(|a| a.code.clone()).collect(),
        };
        let path = self.dir.join(FILE);
        let temp = self.dir.join(format!("{FILE}.new"));
        let written = serde_json::to_vec_pretty(&store)
            .map_err(std::io::Error::other)
            .and_then(|b| write_private(&temp, &b))
            .and_then(|()| std::fs::rename(&temp, &path));
        if let Err(e) = written {
            log::warn!("friends: saving {}: {e}", path.display());
        }
    }

    /// Whether there is anything to do on the network.
    fn active(&self) -> bool {
        self.identity.is_some() && (self.code.is_some() || !self.friends.is_empty() || !self.asked.is_empty())
    }

    /// The player's key and code, made the first time.
    fn identity(&mut self) -> &Identity {
        if self.identity.is_none() {
            self.identity = Some(Identity::generate());
            self.code = Some(crypto::new_code());
            self.save();
            log::info!("friends: a new key and friend code");
        }
        self.identity.as_ref().unwrap()
    }

    fn add_friend(&mut self, key: Key, name: &str, id: u64, since: u64) -> bool {
        let Some(identity) = self.identity.as_ref() else { return false };
        let Some(shared) = identity.shared(&key) else { return false };
        if key == *identity.public() || self.friends.len() >= MAX_FRIENDS {
            return false;
        }
        self.blocked.remove(&key);
        self.friends.entry(key).or_insert(Friend {
            key,
            shared,
            name: clean_name(name),
            id,
            since,
            transport: None,
            previous: None,
            presence: None,
            presence_sent: None,
            last_time: 0,
            outbox: Vec::new(),
            next_seq: 1,
            seen: VecDeque::new(),
            invited: None,
            contacted: HashMap::new(),
        });
        self.generation += 1;
        true
    }

    fn drop_channel(&mut self, transport: Option<Transport>) {
        if let Some(t) = transport {
            self.channels.remove(&t.index);
        }
    }

    fn remove_friend(&mut self, key: &Key, out: &mut Out) {
        let Some(mut f) = self.friends.remove(key) else { return };
        if let Some(t) = f.transport.as_mut() {
            out.push(t.addr, seal(t, &Message::Removed));
        }
        self.drop_channel(f.transport);
        self.drop_channel(f.previous);
        self.handshakes.retain(|_, (_, o)| *o != Owner::Friend(*key));
        self.generation += 1;
        self.save();
    }

    // ---- what the player tells ----

    fn presence_for_friends(&self, lobbies: Vec<String>) -> Presence {
        let mut p = self.local.clone();
        p.lobbies = if p.playing == Playing::Online { lobbies } else { Vec::new() };
        if p.playing != Playing::Online {
            p.session = None;
        }
        p
    }

    // ---- time ----

    /// The DHT lookups wanted now: key, options.
    fn lookups(&self, unix: u64) -> Vec<(dht::Id, dht::Options)> {
        let mut keys = Vec::new();
        if !self.active() {
            return keys;
        }
        let epoch = crypto::epoch(unix);
        for f in self.friends.values() {
            let refresh = if f.connected() { LOOKUP_CONNECTED } else { LOOKUP_ALONE };
            keys.push((crypto::meeting_key(&f.shared, epoch), dht::Options { announce: true, refresh }));
        }
        if let Some(code) = &self.code {
            keys.push((crypto::code_key(code), dht::Options { announce: true, refresh: LOOKUP_CONNECTED }));
        }
        for a in self.asked.iter().filter(|a| a.transport.is_none() && !a.declined) {
            keys.push((crypto::code_key(&a.code), dht::Options { announce: false, refresh: LOOKUP_ALONE }));
        }
        keys
    }

    pub(crate) fn tick(&mut self, now: Instant, lobbies: Vec<String>, port: u16, out: &mut Out) {
        if self.identity.is_none() {
            return;
        }
        // Handshakes not answered.
        self.handshakes.retain(|_, (h, _)| now - h.started < HANDSHAKE);
        let presence = self.presence_for_friends(lobbies);
        let me = *self.identity.as_ref().unwrap().public();
        let keys: Vec<Key> = self.friends.keys().copied().collect();
        for key in keys {
            let f = self.friends.get_mut(&key).unwrap();
            if f.previous.as_ref().is_some_and(|t| now - t.heard > HANDSHAKE * 2) {
                let t = f.previous.take();
                self.drop_channel(t);
            }
            let f = self.friends.get_mut(&key).unwrap();
            let Some(t) = f.transport.as_mut() else { continue };
            if now - t.heard > SILENCE {
                log::info!("friends: {} is offline", f.name);
                f.presence = None;
                f.presence_sent = None;
                let t = f.transport.take();
                self.drop_channel(t);
                self.generation += 1;
                continue;
            }
            // Messages not acknowledged go again, a few times.
            let mut resend = Vec::new();
            f.outbox.retain_mut(|(_, d, sent, tries)| {
                if now - *sent >= RESEND {
                    *sent = now;
                    *tries += 1;
                    resend.push(d.clone());
                }
                *tries <= TRIES
            });
            for d in resend {
                out.push(t.addr, t.seal(&d, now));
            }
            let stale = f.presence_sent.as_ref().is_none_or(|(at, p)| *p != presence || now - *at > PRESENCE);
            if stale {
                f.presence_sent = Some((now, presence.clone()));
                out.push(t.addr, seal(t, &Message::Presence(presence.clone())));
            } else if now - t.sent > PING {
                out.push(t.addr, seal(t, &Message::Ping));
            }
            // Fresh keys now and then: the side that starts handshakes.
            let (addr, old) = (t.addr, now - t.established > REKEY);
            if old && me < key && !self.handshakes.values().any(|(_, o)| *o == Owner::Friend(key)) {
                self.contact(key, addr, false, now, out);
            }
        }
        // Requests this player made: again while unanswered.
        let request = Message::Request { name: self.local.name.clone(), id: self.local.id };
        for a in &mut self.asked {
            let Some(t) = a.transport.as_mut() else { continue };
            if now - t.heard > SILENCE {
                let t = a.transport.take().unwrap();
                self.channels.remove(&t.index);
                continue;
            }
            if a.asked.is_none_or(|at| now - at > ASK_AGAIN) {
                a.asked = Some(now);
                out.push(t.addr, seal(t, &request));
            }
        }
        // Requests to this player: their channels stay open while it decides.
        let mut gone = Vec::new();
        for (key, r) in &mut self.requests {
            if now - r.transport.heard > SILENCE {
                gone.push(*key);
            } else if now - r.transport.sent > PING {
                out.push(r.transport.addr, seal(&mut r.transport, &Message::Ping));
            }
        }
        for key in gone {
            if let Some(r) = self.requests.remove(&key) {
                self.channels.remove(&r.transport.index);
                self.generation += 1;
            }
        }
        // The local network: probes for friends and codes not connected.
        if self.probed.is_none_or(|t| now - t > PROBE_EVERY) {
            self.probed = Some(now);
            let tags = self.wanted_tags(unix());
            if !tags.is_empty() {
                let d = self.probe(&tags);
                for p in crate::p2p::PORTS {
                    out.send.push((SocketAddrV4::new(Ipv4Addr::BROADCAST, p), d.clone()));
                    if p != port {
                        out.send.push((SocketAddrV4::new(Ipv4Addr::LOCALHOST, p), d.clone()));
                    }
                }
                if self.swept.is_none_or(|t| now - t > SWEEP_EVERY) {
                    self.swept = Some(now);
                    if let Some(own) = crate::p2p::lan_address() {
                        let [a, b, c, _] = own.octets();
                        for x in 1..255 {
                            let ip = Ipv4Addr::new(a, b, c, x);
                            if ip != own {
                                out.send.push((SocketAddrV4::new(ip, crate::p2p::PORTS.start), d.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    /// The tags this player looks for: friends not connected, codes asked.
    fn wanted_tags(&self, unix: u64) -> Vec<Tag> {
        if !self.active() {
            return Vec::new();
        }
        let epoch = crypto::epoch(unix);
        let mut tags: Vec<Tag> =
            self.friends.values().filter(|f| !f.connected()).map(|f| crypto::meeting_tag(&f.shared, epoch)).collect();
        tags.extend(self.asked.iter().filter(|a| a.transport.is_none() && !a.declined).map(|a| crypto::code_tag(&a.code)));
        tags
    }

    fn probe(&self, tags: &[Tag]) -> Vec<u8> {
        let mut all = vec![self.probe_id];
        all.extend_from_slice(tags);
        session::probe(&all)
    }

    /// An address the DHT gave for `key`.
    pub(crate) fn found(&mut self, key: dht::Id, addr: SocketAddrV4, now: Instant, out: &mut Out) {
        if self.own.contains(&addr) || !self.active() {
            return;
        }
        let epoch = crypto::epoch(unix());
        if let Some(f) = self.friends.values().find(|f| crypto::meeting_key(&f.shared, epoch) == key) {
            let k = f.key;
            self.contact(k, addr, false, now, out);
            return;
        }
        if let Some(i) = self.asked.iter().position(|a| crypto::code_key(&a.code) == key) {
            self.ask_at(i, addr, now, out);
        }
    }

    /// Reaches a friend at `addr`: the side with the lower key starts the
    /// handshake, the other probes there so that it does. `again`: even if
    /// connected (the friend probing says it is not: it started over).
    fn contact(&mut self, key: Key, addr: SocketAddrV4, again: bool, now: Instant, out: &mut Out) {
        let Some(identity) = self.identity.as_ref() else { return };
        let Some(f) = self.friends.get_mut(&key) else { return };
        let starts = identity.public() < &key;
        // One handshake at a time (each address the friend answers at would
        // open a channel of its own): the next address once it fails.
        let busy = starts && self.handshakes.values().any(|(_, o)| *o == Owner::Friend(key));
        let due = f.transport.as_ref().is_none_or(|t| now - t.established > REKEY);
        if busy || !due && !again || f.contacted.get(&addr).is_some_and(|t| now - *t < CONTACT) {
            return;
        }
        f.contacted.insert(addr, now);
        f.contacted.retain(|_, t| now - *t < CONTACT * 4);
        if starts {
            let time = unix_ms().to_be_bytes();
            if let Some((h, d)) = Handshake::friend(identity, &key, &time, addr, now) {
                self.handshakes.insert(h.index, (h, Owner::Friend(key)));
                out.send.push((addr, d));
            }
        } else {
            let tag = crypto::meeting_tag(&f.shared, crypto::epoch(unix()));
            let d = self.probe(&[tag]);
            out.send.push((addr, d));
        }
    }

    /// Asks a code's owner, found at `addr`.
    fn ask_at(&mut self, i: usize, addr: SocketAddrV4, now: Instant, out: &mut Out) {
        let Some(identity) = self.identity.as_ref() else { return };
        let a = &mut self.asked[i];
        let busy = self.handshakes.values().any(|(_, o)| *o == Owner::Code(a.code.clone()));
        if busy || a.transport.is_some() || a.declined || a.contacted.get(&addr).is_some_and(|t| now - *t < CONTACT) {
            return;
        }
        a.contacted.insert(addr, now);
        a.contacted.retain(|_, t| now - *t < CONTACT * 4);
        if let Some((h, d)) = Handshake::code(identity, &a.code, addr, now) {
            self.handshakes.insert(h.index, (h, Owner::Code(a.code.clone())));
            out.send.push((addr, d));
        }
    }

    // ---- datagrams ----

    pub(crate) fn receive(&mut self, from: SocketAddrV4, kind: u8, d: &[u8], now: Instant, out: &mut Out) {
        if self.identity.is_none() {
            return;
        }
        match kind {
            session::PROBE => self.on_probe(from, d, now, out),
            session::HS1 if d.len() > 5 => {
                let Some(pattern) = Pattern::from_byte(d[0]) else { return };
                let index = u32::from_be_bytes(d[1..5].try_into().unwrap());
                if self.allowed(from, now) {
                    self.on_first(from, pattern, index, &d[5..], now, out);
                }
            }
            session::HS2 | session::HS3 if d.len() > 9 => {
                let sender = u32::from_be_bytes(d[1..5].try_into().unwrap());
                let receiver = u32::from_be_bytes(d[5..9].try_into().unwrap());
                self.on_next(from, sender, receiver, &d[9..], now, out);
            }
            session::DATA if d.len() > 12 => {
                let index = u32::from_be_bytes(d[..4].try_into().unwrap());
                self.on_data(from, index, &d[4..], now, out);
            }
            _ => {}
        }
    }

    /// Whether a handshake from `from` is answered: at most so many per
    /// address and in all.
    fn allowed(&mut self, from: SocketAddrV4, now: Instant) -> bool {
        if now - self.answered_all.0 > RATE_WINDOW {
            self.answered_all = (now, 0);
        }
        self.answered.retain(|_, (t, _)| now - *t < RATE_WINDOW);
        let (_, n) = self.answered.entry(*from.ip()).or_insert((now, 0));
        *n += 1;
        self.answered_all.1 += 1;
        *n <= RATE_PER_ADDRESS && self.answered_all.1 <= RATE_ALL
    }

    fn on_probe(&mut self, from: SocketAddrV4, d: &[u8], now: Instant, out: &mut Out) {
        let tags: Vec<Tag> = d.as_chunks::<8>().0.to_vec();
        let Some((id, tags)) = tags.split_first() else { return };
        if *id == self.probe_id {
            self.own.insert(from);
            return;
        }
        let epoch = crypto::epoch(unix());
        // A friend (this epoch's tag or the last one's: clocks differ).
        let friends: Vec<Key> = self
            .friends
            .values()
            .filter(|f| {
                tags.iter().any(|t| *t == crypto::meeting_tag(&f.shared, epoch) || *t == crypto::meeting_tag(&f.shared, epoch.saturating_sub(1)))
            })
            .map(|f| f.key)
            .collect();
        for key in friends {
            self.contact(key, from, true, now, out);
        }
        // Someone with this player's code: told where it is, it starts.
        if let Some(code) = &self.code {
            let tag = crypto::code_tag(code);
            if tags.contains(&tag) && self.allowed(from, now) {
                let d = self.probe(&[tag]);
                out.send.push((from, d));
            }
        }
        // The owner of a code asked.
        if let Some(i) = self.asked.iter().position(|a| tags.contains(&crypto::code_tag(&a.code))) {
            self.ask_at(i, from, now, out);
        }
    }

    /// Message 1 of a handshake.
    fn on_first(&mut self, from: SocketAddrV4, pattern: Pattern, index: u32, msg: &[u8], now: Instant, out: &mut Out) {
        let identity = self.identity.as_ref().unwrap();
        match pattern {
            Pattern::Friend => {
                let Some((mut h, payload)) = Handshake::answer(identity, pattern, None, index, msg, from, now) else { return };
                let Some(key) = h.remote_key() else { return };
                let me = *identity.public();
                let Some(f) = self.friends.get_mut(&key) else { return };
                let Ok(time) = <[u8; 8]>::try_from(payload.as_slice()).map(u64::from_be_bytes) else { return };
                if time <= f.last_time {
                    return;
                }
                // Both started at once: the lower key's handshake goes on.
                let ours = self.handshakes.iter().find(|(_, (_, o))| *o == Owner::Friend(key)).map(|(i, _)| *i);
                if ours.is_some() && me < key {
                    return;
                }
                if let Some(i) = ours {
                    self.handshakes.remove(&i);
                }
                f.last_time = time;
                let Some(d) = h.write(&[]) else { return };
                out.send.push((from, d));
                if let Some(t) = h.into_transport(now) {
                    self.open_friend(key, t, now, out);
                }
            }
            Pattern::Code => {
                let Some(code) = &self.code else { return };
                let psk = crypto::code_psk(code);
                if self.handshakes.values().filter(|(_, o)| matches!(o, Owner::Asker(_))).count() >= MAX_REQUESTS {
                    return;
                }
                let Some((mut h, _)) = Handshake::answer(identity, pattern, Some(&psk), index, msg, from, now) else { return };
                let Some(d) = h.write(&[]) else { return };
                out.send.push((from, d));
                self.handshakes.insert(h.index, (h, Owner::Asker(None)));
            }
        }
    }

    /// Message 2 or 3.
    fn on_next(&mut self, from: SocketAddrV4, sender: u32, receiver: u32, msg: &[u8], now: Instant, out: &mut Out) {
        let Some((mut h, owner)) = self.handshakes.remove(&receiver) else { return };
        if h.remote_index == 0 {
            h.remote_index = sender;
        }
        if h.remote_index != sender || h.read(msg).is_none() {
            return;
        }
        h.to = from;
        if !h.finished() {
            // XX, message 2 read: message 3 finishes it.
            let Some(d) = h.write(&[]) else { return };
            out.send.push((from, d));
        }
        let Some(key) = h.remote_key() else { return };
        let Some(t) = h.into_transport(now) else { return };
        match owner {
            Owner::Friend(k) if k == key => self.open_friend(key, t, now, out),
            Owner::Code(code) => {
                // The owner of the code: a friend already, or asked now.
                if self.friends.contains_key(&key) {
                    self.asked.retain(|a| a.code != code);
                    self.open_friend(key, t, now, out);
                    self.save();
                    return;
                }
                let Some(a) = self.asked.iter_mut().find(|a| a.code == code && a.transport.is_none()) else { return };
                log::info!("friends: reached the owner of a code");
                self.channels.insert(t.index, Owner::Code(code));
                a.asked = None;
                a.transport = Some(t);
                self.generation += 1;
            }
            Owner::Asker(_) => {
                if self.blocked.contains(&key) {
                    return;
                }
                if self.friends.contains_key(&key) {
                    self.open_friend(key, t, now, out);
                    return;
                }
                if !self.requests.contains_key(&key) && self.requests.len() >= MAX_REQUESTS {
                    return;
                }
                self.channels.insert(t.index, Owner::Asker(Some(key)));
                let request = Request { name: String::new(), id: 0, transport: t, asked: false };
                if let Some(old) = self.requests.insert(key, request) {
                    self.channels.remove(&old.transport.index);
                }
            }
            Owner::Friend(_) => {}
        }
    }

    /// A friend's new channel.
    fn open_friend(&mut self, key: Key, t: Transport, now: Instant, out: &mut Out) {
        let lobbies = crate::p2p::get().map(|p| p.lobbies()).unwrap_or_default();
        let presence = self.presence_for_friends(lobbies);
        let Some(f) = self.friends.get_mut(&key) else { return };
        if f.transport.is_none() {
            log::info!("friends: {} is online", f.name);
        }
        self.channels.insert(t.index, Owner::Friend(key));
        let previous = f.previous.take();
        f.previous = f.transport.replace(t);
        let t = f.transport.as_mut().unwrap();
        f.presence_sent = Some((now, presence.clone()));
        out.push(t.addr, seal(t, &Message::Presence(presence)));
        // Messages still waiting go on the new channel.
        for (_, d, sent, _) in &mut f.outbox {
            *sent = now;
            out.push(t.addr, t.seal(d, now));
        }
        self.drop_channel(previous);
        self.generation += 1;
    }

    fn on_data(&mut self, from: SocketAddrV4, index: u32, d: &[u8], now: Instant, out: &mut Out) {
        let Some(owner) = self.channels.get(&index).cloned() else { return };
        let payload = match &owner {
            Owner::Friend(key) => {
                let Some(f) = self.friends.get_mut(key) else { return };
                let t = [f.transport.as_mut(), f.previous.as_mut()].into_iter().flatten().find(|t| t.index == index);
                t.and_then(|t| t.open(d, from, now))
            }
            Owner::Asker(Some(key)) => self.requests.get_mut(key).and_then(|r| r.transport.open(d, from, now)),
            Owner::Code(code) => {
                self.asked.iter_mut().find(|a| a.code == *code).and_then(|a| a.transport.as_mut()).and_then(|t| t.open(d, from, now))
            }
            Owner::Asker(None) => None,
        };
        let Some(message) = payload.and_then(|p| serde_json::from_slice::<Message>(&p).ok()) else { return };
        match owner {
            Owner::Friend(key) => self.on_friend(key, message, now, out),
            Owner::Asker(Some(key)) => self.on_asker(key, message, now, out),
            Owner::Code(code) => self.on_owner(&code, message, now, out),
            Owner::Asker(None) => {}
        }
    }

    fn on_friend(&mut self, key: Key, message: Message, now: Instant, out: &mut Out) {
        let Some(f) = self.friends.get_mut(&key) else { return };
        match message {
            Message::Presence(p) => {
                let p = p.bounded();
                if f.presence.as_ref() != Some(&p) {
                    if !p.name.is_empty() && p.name != f.name {
                        f.name = p.name.clone();
                    }
                    let id_changed = p.id != f.id;
                    f.id = p.id;
                    f.presence = Some(p);
                    self.generation += 1;
                    if id_changed {
                        self.save();
                    }
                }
            }
            Message::Game { seq, text } => {
                if !f.first_time(seq, out) || text.len() > MAX_TEXT {
                    return;
                }
                log::info!("friends: a game event from {}", f.name);
                self.events.push_back(GameEvent { from: f.id, name: f.name.clone(), text });
                self.events.truncate(32);
            }
            Message::Invite { seq } => {
                if !f.first_time(seq, out) || f.invited.is_some_and(|t| now - t < INVITE_AGAIN) {
                    return;
                }
                log::info!("friends: {} invites this player", f.name);
                f.invited = Some(now);
                self.generation += 1;
                if self.local.playing == Playing::Online && !self.invites.contains(&f.id) {
                    self.invites.push_back(f.id);
                }
            }
            Message::Ack { seq } => f.outbox.retain(|(s, ..)| *s != seq),
            Message::Removed => {
                log::info!("friends: {} is no friend any more", f.name);
                self.remove_friend(&key, out);
            }
            // A friend asking again (with the code): yes.
            Message::Request { .. } => {
                let accept = Message::Accept { name: self.local.name.clone(), id: self.local.id };
                if let Some(t) = self.friends.get_mut(&key).and_then(|f| f.transport.as_mut()) {
                    out.push(t.addr, seal(t, &accept));
                }
            }
            Message::Accept { .. } | Message::Decline | Message::Ping => {}
        }
    }

    fn on_asker(&mut self, key: Key, message: Message, _now: Instant, out: &mut Out) {
        let Some(r) = self.requests.get_mut(&key) else { return };
        if let Message::Request { name, id } = message {
            let name = clean_name(&name);
            if !r.asked || r.name != name {
                log::info!("friends: {name} asks to be friends");
                r.name = name;
                r.id = id;
                r.asked = true;
                self.generation += 1;
            }
            let _ = out;
        }
    }

    fn on_owner(&mut self, code: &str, message: Message, now: Instant, out: &mut Out) {
        let Some(i) = self.asked.iter().position(|a| a.code == code) else { return };
        match message {
            Message::Accept { name, id } => {
                let mut a = self.asked.remove(i);
                let Some(t) = a.transport.take() else { return };
                let key = t.remote_key;
                self.channels.remove(&t.index);
                if self.add_friend(key, &name, id, unix()) {
                    log::info!("friends: {} said yes", clean_name(&name));
                    self.open_friend(key, t, now, out);
                }
                self.save();
            }
            Message::Decline => {
                let a = &mut self.asked[i];
                a.declined = true;
                let t = a.transport.take();
                self.drop_channel(t);
                self.generation += 1;
                self.save();
            }
            _ => {}
        }
    }

    // ---- the player's answers ----

    fn answer(&mut self, key: &Key, yes: bool, now: Instant, out: &mut Out) {
        let Some(mut r) = self.requests.remove(key) else { return };
        if yes {
            self.channels.remove(&r.transport.index);
            if self.add_friend(*key, &r.name, r.id, unix()) {
                let accept = Message::Accept { name: self.local.name.clone(), id: self.local.id };
                out.push(r.transport.addr, seal(&mut r.transport, &accept));
                self.open_friend(*key, r.transport, now, out);
            }
        } else {
            out.push(r.transport.addr, seal(&mut r.transport, &Message::Decline));
            self.channels.remove(&r.transport.index);
            self.blocked.insert(*key);
        }
        self.generation += 1;
        self.save();
    }

    /// Sends the game's event to the friend with gamer id `id`; false if no
    /// such friend is connected.
    fn send_game(&mut self, id: u64, text: &str, now: Instant, out: &mut Out) -> bool {
        let Some(f) = self.friends.values_mut().find(|f| f.id == id) else { return false };
        f.send_reliably(|seq| Message::Game { seq, text: text.to_owned() }, now, out)
    }

    /// Invites the friend with gamer id `id` to this player's session.
    fn send_invite(&mut self, id: u64, now: Instant, out: &mut Out) -> bool {
        let Some(f) = self.friends.values_mut().find(|f| f.id == id) else { return false };
        log::info!("friends: inviting {}", f.name);
        f.send_reliably(|seq| Message::Invite { seq }, now, out)
    }
}

fn seal(t: &mut Transport, m: &Message) -> Option<Vec<u8>> {
    let d = serde_json::to_vec(m).ok()?;
    t.seal(&d, Instant::now())
}

/// Writes a file only the player can read (where the system has owners).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

// ---- running on the player's socket ------------------------------------------

static SERVICE: Mutex<Option<Service>> = Mutex::new(None);
/// The friends' lookups on the DHT, as last set.
static LOOKUPS: Mutex<Vec<dht::Id>> = Mutex::new(Vec::new());

fn send(p2p: &crate::p2p::P2p, out: Out) {
    for (to, d) in out.send {
        p2p.send_to(to, &d);
    }
}

/// Runs `f` on the service, then sends what it wants sent.
fn with<R>(f: impl FnOnce(&mut Service, &mut Out) -> R) -> Option<R> {
    let mut out = Out::default();
    let r = {
        let mut service = SERVICE.lock().unwrap();
        f(service.as_mut()?, &mut out)
    };
    if !out.send.is_empty()
        && let Some(p2p) = crate::p2p::get()
    {
        send(p2p, out);
    }
    Some(r)
}

/// Every quarter of a second, on the network thread.
pub(crate) fn tick(p2p: &crate::p2p::P2p, now: Instant) {
    let lobbies = p2p.lobbies();
    let mut out = Out::default();
    let wanted = {
        let mut service = SERVICE.lock().unwrap();
        let Some(s) = service.as_mut() else { return };
        s.tick(now, lobbies, p2p.port(), &mut out);
        s.lookups(unix())
    };
    send(p2p, out);
    let mut current = LOOKUPS.lock().unwrap();
    let keys: Vec<dht::Id> = wanted.iter().map(|(k, _)| *k).collect();
    p2p.dht(|dht| {
        for old in current.iter().filter(|k| !keys.contains(k)) {
            dht.remove(old);
        }
        // Their options change as friends connect and leave.
        for (key, options) in &wanted {
            dht.add(*key, *options);
        }
    });
    *current = keys;
}

/// A datagram of the friends' kinds (after the magic and the kind).
pub(crate) fn receive(p2p: &crate::p2p::P2p, from: SocketAddrV4, kind: u8, d: &[u8]) {
    let mut out = Out::default();
    if let Some(s) = SERVICE.lock().unwrap().as_mut() {
        s.receive(from, kind, d, Instant::now(), &mut out);
    }
    send(p2p, out);
}

/// An address the DHT gave for a key not a lobby's.
pub(crate) fn found(p2p: &crate::p2p::P2p, key: dht::Id, addr: SocketAddrV4) {
    let mut out = Out::default();
    if let Some(s) = SERVICE.lock().unwrap().as_mut() {
        s.found(key, addr, Instant::now(), &mut out);
    }
    send(p2p, out);
}

// ---- for the application -------------------------------------------------------

/// Starts the friends of the player whose files are in `user_dir` (the
/// network once there is something to do on it). `name`: the player's name.
pub fn start(user_dir: &Path, name: &str) {
    let id = crate::online::gamer_id_in(user_dir);
    let mut service = SERVICE.lock().unwrap();
    if service.is_some() {
        return;
    }
    let s = Service::load(user_dir, name, id);
    let active = s.active();
    *service = Some(s);
    drop(service);
    if active {
        start_network();
    }
}

fn start_network() {
    if let Err(e) = crate::p2p::start() {
        log::warn!("friends: no network: {e}");
    }
}

/// The player's name as its friends see it.
pub fn set_name(name: &str) {
    with(|s, _| s.local.name = clean_name(name));
}

/// What the player is doing.
pub(crate) fn set_playing(playing: Playing) {
    with(|s, _| {
        s.local.playing = playing;
        if playing != Playing::Online {
            s.local.session = None;
        }
    });
}

/// A friend as the application shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FriendInfo {
    /// Its key (hex): what the application names it by.
    pub key: String,
    pub name: String,
    /// What it is doing, when connected.
    pub playing: Option<Playing>,
    /// The lobby to join it in (playing GTA Online).
    pub lobby: Option<String>,
    /// It invited this player to its session (a few minutes ago at most).
    pub invited: bool,
}

/// A code this player asked with.
#[derive(Clone, Debug, PartialEq)]
pub struct AskedInfo {
    pub code: String,
    /// The owner was reached and asked.
    pub reached: bool,
    pub declined: bool,
}

/// Everything the application shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The player's friend code (shown with dashes), once made.
    pub code: Option<String>,
    pub friends: Vec<FriendInfo>,
    /// Requests to this player: key, name.
    pub requests: Vec<(String, String)>,
    pub asked: Vec<AskedInfo>,
}

/// A number that changes with what `snapshot` shows.
pub fn generation() -> u64 {
    with(|s, _| s.generation).unwrap_or(0)
}

pub fn snapshot() -> Snapshot {
    with(|s, _| {
        let mut friends: Vec<FriendInfo> = s
            .friends
            .values()
            .map(|f| {
                let presence = f.transport.as_ref().and(f.presence.as_ref());
                FriendInfo {
                    key: crypto::hex(&f.key),
                    name: if f.name.is_empty() { "?".into() } else { f.name.clone() },
                    playing: presence.map(|p| p.playing),
                    lobby: presence.filter(|p| p.playing == Playing::Online).and_then(|p| p.lobbies.first().cloned()),
                    invited: presence.is_some() && f.invited.is_some_and(|t| t.elapsed() < INVITED),
                }
            })
            .collect();
        // Playing first, then by name.
        friends.sort_by(|a, b| (b.playing.is_some(), a.name.to_lowercase()).cmp(&(a.playing.is_some(), b.name.to_lowercase())));
        let mut requests: Vec<(String, String)> =
            s.requests.iter().filter(|(_, r)| r.asked).map(|(k, r)| (crypto::hex(k), r.name.clone())).collect();
        requests.sort_by(|a, b| a.1.cmp(&b.1));
        Snapshot {
            code: s.code.as_deref().map(crypto::format_code),
            friends,
            requests,
            asked: s
                .asked
                .iter()
                .map(|a| AskedInfo { code: crypto::format_code(&a.code), reached: a.transport.is_some(), declined: a.declined })
                .collect(),
        }
    })
    .unwrap_or_default()
}

/// The player's friend code, made the first time (the network starts).
pub fn my_code() -> Option<String> {
    let code = with(|s, _| {
        s.identity();
        s.code.as_deref().map(crypto::format_code)
    })?;
    start_network();
    code
}

/// A new friend code: the old one no longer reaches the player.
pub fn new_code() {
    with(|s, _| {
        s.identity();
        s.code = Some(crypto::new_code());
        s.generation += 1;
        s.save();
    });
}

/// Why a code was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddError {
    /// Not a code (mistyped).
    Invalid,
    /// The player's own.
    Own,
    /// Asked already.
    Asked,
}

/// Asks the owner of a friend code to be friends (it says yes or no).
pub fn ask(code: &str) -> Result<(), AddError> {
    let code = crypto::parse_code(code).ok_or(AddError::Invalid)?;
    with(|s, _| {
        s.identity();
        if s.code.as_deref() == Some(code.as_str()) {
            return Err(AddError::Own);
        }
        if let Some(a) = s.asked.iter_mut().find(|a| a.code == code) {
            if !a.declined {
                return Err(AddError::Asked);
            }
            a.declined = false;
        } else {
            s.asked.push(Ask { code, transport: None, asked: None, declined: false, contacted: HashMap::new() });
        }
        s.generation += 1;
        s.save();
        Ok(())
    })
    .unwrap_or(Err(AddError::Invalid))?;
    start_network();
    Ok(())
}

/// Stops asking with a code (or forgets a no).
pub fn cancel(code: &str) {
    let Some(code) = crypto::parse_code(code) else { return };
    with(|s, _| {
        if let Some(i) = s.asked.iter().position(|a| a.code == code) {
            let a = s.asked.remove(i);
            s.drop_channel(a.transport);
            s.generation += 1;
            s.save();
        }
    });
}

/// Answers a request (by the asker's key).
pub fn answer(key: &str, yes: bool) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, out| s.answer(&key, yes, Instant::now(), out));
}

/// Joins a friend (by its key) playing GTA Online: in the game's session
/// it plays in, once this player's game is in GTA Online (game.rs).
pub fn follow(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, _| s.follow = Some((key, Instant::now())));
}

/// Invites a friend (by its key) to this player's session in GTA Online:
/// its game asks it whether to join.
pub fn invite(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, out| {
        let id = s.friends.get(&key).map_or(0, |f| f.id);
        s.send_invite(id, Instant::now(), out);
    });
}

/// No longer friends (the friend is told when connected).
pub fn remove(key: &str) {
    let Some(key) = crypto::unhex::<32>(key) else { return };
    with(|s, out| s.remove_friend(&key, out));
}

// ---- for the game (game.rs) -------------------------------------------------------

/// How long a friend is waited for to be joined.
const FOLLOW: Duration = Duration::from_secs(180);

/// A friend as the game lists it.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct GameFriend {
    pub name: String,
    pub id: u64,
    /// Connected (playing Glue 5, whatever it plays).
    pub online: bool,
    /// In a session of GTA Online, which can be joined.
    pub in_session: bool,
}

/// The friends the game lists: connected first.
pub(crate) fn game_friends() -> Vec<GameFriend> {
    with(|s, _| {
        let mut list: Vec<GameFriend> = s
            .friends
            .values()
            .filter(|f| f.id != 0)
            .map(|f| {
                let presence = f.transport.as_ref().and(f.presence.as_ref());
                GameFriend {
                    name: f.name.clone(),
                    id: f.id,
                    online: presence.is_some(),
                    in_session: presence.is_some_and(|p| p.playing == Playing::Online && p.session.is_some()),
                }
            })
            .collect();
        list.sort_by(|a, b| (b.online, &a.name).cmp(&(a.online, &b.name)));
        list
    })
    .unwrap_or_default()
}

pub(crate) fn is_friend(id: u64) -> bool {
    id != 0 && with(|s, _| s.friends.values().any(|f| f.id == id)).unwrap_or(false)
}

/// Sends the game's event to the friends among `ids`: how many it went to.
pub(crate) fn send_game_event(ids: &[u64], text: &str) -> usize {
    with(|s, out| ids.iter().filter(|id| s.send_game(**id, text, Instant::now(), out)).count()).unwrap_or(0)
}

/// The game's events from friends, received since the last call.
pub(crate) fn take_game_events() -> Vec<GameEvent> {
    with(|s, _| s.events.drain(..).collect()).unwrap_or_default()
}

/// The session this player is in (as the engine writes its info).
pub(crate) fn set_session(session: Option<String>) {
    with(|s, _| s.local.session = session);
}

/// The session a friend (gamer id) plays in, if it says.
pub(crate) fn session_of(id: u64) -> Option<String> {
    with(|s, _| {
        let f = s.friends.values().find(|f| f.id == id)?;
        f.transport.as_ref()?;
        f.presence.as_ref().filter(|p| p.playing == Playing::Online)?.session.clone()
    })
    .flatten()
}

/// Gets ready to join a friend (gamer id): its lobbies, its players
/// introduced by it.
pub(crate) fn reach(id: u64) {
    let target = with(|s, _| {
        let f = s.friends.values().find(|f| f.id == id)?;
        let addr = f.transport.as_ref()?.addr;
        let p = f.presence.as_ref().filter(|p| p.playing == Playing::Online)?;
        Some((addr, p.lobbies.clone()))
    })
    .flatten();
    let (Some((addr, lobbies)), Some(p2p)) = (target, crate::p2p::get()) else { return };
    for lobby in lobbies {
        p2p.visit_lobby(&lobby, addr);
    }
}

/// The friend (gamer id) to join, while it is waited for.
pub(crate) fn following() -> Option<u64> {
    with(|s, _| {
        let (key, since) = s.follow?;
        if since.elapsed() > FOLLOW || !s.friends.contains_key(&key) {
            s.follow = None;
            return None;
        }
        Some(s.friends[&key].id).filter(|id| *id != 0)
    })
    .flatten()
}

/// Invites the friends among `ids` (gamer ids) to this player's session:
/// how many it went to.
pub(crate) fn send_invites(ids: &[u64]) -> usize {
    with(|s, out| ids.iter().filter(|id| s.send_invite(**id, Instant::now(), out)).count()).unwrap_or(0)
}

/// The friends (gamer ids) who invited this player to their sessions since
/// the last call.
pub(crate) fn take_invites() -> Vec<u64> {
    with(|s, _| s.invites.drain(..).collect()).unwrap_or_default()
}

pub(crate) fn stop_following() {
    with(|s, _| s.follow = None);
}

#[cfg(test)]
mod tests;
