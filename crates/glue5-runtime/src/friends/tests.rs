//! Players in memory: their datagrams go straight to each other, the DHT's
//! answers are given by hand.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;

struct Player {
    s: Service,
    addr: SocketAddrV4,
    dir: PathBuf,
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glue5-friends-{}-{name}-{:x}", std::process::id(), crate::p2p::random()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

impl Player {
    fn new(name: &str, port: u16) -> Player {
        let dir = dir(name);
        let mut s = Service::load(&dir, name, u64::from(port) * 1000);
        s.identity();
        Player { s, addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port), dir }
    }

    /// The same player started again (from its file).
    fn restarted(&self) -> Player {
        let s = Service::load(&self.dir, &self.s.local.name, self.s.local.id);
        Player { s, addr: self.addr, dir: self.dir.clone() }
    }

    fn key(&self) -> Key {
        *self.s.identity.as_ref().unwrap().public()
    }

    fn code(&self) -> String {
        self.s.code.clone().unwrap()
    }

    fn connected_to(&self, other: &Player) -> bool {
        self.s.friends.get(&other.key()).is_some_and(|f| f.connected() && f.presence.is_some())
    }
}

/// Delivers datagrams between the players until none is left; `drop`
/// decides which are lost. The players' ticks run every quarter second.
fn run(players: &mut [&mut Player], first: Out, from: SocketAddrV4, now: &mut Instant, ticks: u32, drop: &dyn Fn(&[u8]) -> bool) {
    let mut queue: Vec<(SocketAddrV4, SocketAddrV4, Vec<u8>)> = first.send.into_iter().map(|(to, d)| (from, to, d)).collect();
    for _ in 0..ticks {
        let mut rounds = 0;
        while !queue.is_empty() {
            rounds += 1;
            assert!(rounds < 1000, "datagrams going round");
            for (from, to, d) in std::mem::take(&mut queue) {
                if drop(&d) {
                    continue;
                }
                let Some(p) = players.iter_mut().find(|p| p.addr == to) else { continue };
                let mut out = Out::default();
                assert_eq!(d[..3], crate::p2p::MAGIC);
                p.s.receive(from, d[3], &d[4..], *now, &mut out);
                queue.extend(out.send.into_iter().map(|(t, d)| (p.addr, t, d)));
            }
        }
        *now += Duration::from_millis(250);
        for p in players.iter_mut() {
            let mut out = Out::default();
            p.s.tick(*now, Vec::new(), &[], p.addr.port(), &mut out);
            queue.extend(out.send.into_iter().map(|(t, d)| (p.addr, t, d)));
        }
    }
}

fn keep(_: &[u8]) -> bool {
    false
}

/// `b` asks `a` with `a`'s code, found on the DHT; `a` answers.
fn befriend(a: &mut Player, b: &mut Player, yes: bool, now: &mut Instant) {
    b.s.asked.push(Ask::new(a.code(), None));
    let mut out = Out::default();
    b.s.found(crypto::code_key(&a.code()), a.addr, *now, &mut out);
    let from = b.addr;
    run(&mut [&mut *a, &mut *b], out, from, now, 4, &keep);
    let key = b.key();
    assert_eq!(a.s.requests.get(&key).map(|r| (r.asked, r.name.as_str())), Some((true, b.s.local.name.as_str())));
    let mut out = Out::default();
    a.s.answer(&key, yes, *now, &mut out);
    let from = a.addr;
    run(&mut [&mut *a, &mut *b], out, from, now, 4, &keep);
}

#[test]
fn a_code_makes_friends_who_then_meet_again() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    assert!(a.connected_to(&b) && b.connected_to(&a));
    assert!(b.s.asked.is_empty());
    assert_eq!(a.s.friends[&b.key()].name, "Bob");
    assert_eq!(b.s.friends[&a.key()].id, 1000);
    assert_eq!(a.s.friends[&b.key()].presence.as_ref().map(|p| p.playing), Some(Playing::Menu));

    // Both start again: whoever finds the other first (the DHT, its
    // meeting key) reaches it, whichever key is lower.
    for finder in [0, 1] {
        let (mut a2, mut b2) = (a.restarted(), b.restarted());
        assert_eq!(a2.s.friends.len(), 1);
        let (finder, other) = if finder == 0 { (&mut a2, &mut b2) } else { (&mut b2, &mut a2) };
        let shared = finder.s.friends.values().next().unwrap().shared;
        let mut out = Out::default();
        finder.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), other.addr, now, &mut out);
        // The higher key probes: the other has to probe back.
        if finder.key() > other.key() {
            let mut back = Out::default();
            other.s.on_probe(finder.addr, &probe_body(&other.s, &shared), now, &mut back);
            let from = other.addr;
            run(&mut [finder, other], back, from, &mut now, 4, &keep);
        } else {
            let from = finder.addr;
            run(&mut [finder, other], out, from, &mut now, 4, &keep);
        }
        assert!(a2.connected_to(&b2) && b2.connected_to(&a2));
    }
}

/// A probe's body (after the kind) for one tag, from someone else.
fn probe_body(_s: &Service, shared: &Key) -> Vec<u8> {
    let mut d = vec![9; 8];
    d.extend_from_slice(&crypto::meeting_tag(shared, crypto::epoch(unix())));
    d
}

#[test]
fn a_no_is_kept() {
    let mut now = Instant::now();
    let (mut a, mut c) = (Player::new("Ann", 1), Player::new("Carl", 3));
    befriend(&mut a, &mut c, false, &mut now);
    assert!(a.s.friends.is_empty() && c.s.friends.is_empty());
    assert!(c.s.asked[0].declined);
    assert!(a.s.blocked.contains(&c.key()));
    // Asking again: not heard.
    c.s.asked[0].declined = false;
    c.s.asked[0].contacted.clear();
    let mut out = Out::default();
    c.s.found(crypto::code_key(&a.code()), a.addr, now, &mut out);
    let from = c.addr;
    run(&mut [&mut a, &mut c], out, from, &mut now, 4, &keep);
    assert!(a.s.requests.is_empty());
}

#[test]
fn strangers_and_replays_are_not_answered() {
    let mut now = Instant::now();
    let (mut a, mut b, mut d) = (Player::new("Ann", 1), Player::new("Bob", 2), Player::new("Dan", 4));
    befriend(&mut a, &mut b, true, &mut now);
    // Dan knows Bob's key but is no friend of his: Bob says nothing.
    let mut out = Out::default();
    let time = unix_ms().to_be_bytes();
    let (h, first) = Handshake::friend(d.s.identity.as_ref().unwrap(), &b.key(), &time, b.addr, now).unwrap();
    d.s.handshakes.insert(h.index, (h, Owner::Friend(b.key())));
    let mut answer = Out::default();
    b.s.receive(d.addr, first[3], &first[4..], now, &mut answer);
    assert!(answer.send.is_empty());
    // A friend's handshake seen before (replayed by someone in between):
    // not answered again.
    let (lower, higher) = if a.key() < b.key() { (&mut a, &mut b) } else { (&mut b, &mut a) };
    let key = higher.key();
    let shared = lower.s.friends[&key].shared;
    lower.s.friends.get_mut(&key).unwrap().contacted.clear();
    lower.s.found(crypto::meeting_key(&shared, crypto::epoch(unix())), higher.addr, now + REKEY * 2, &mut out);
    let hs1 = out.send.iter().find(|(_, d)| d[3] == session::HS1).unwrap().1.clone();
    let mut first_answer = Out::default();
    higher.s.receive(lower.addr, hs1[3], &hs1[4..], now, &mut first_answer);
    assert_eq!(first_answer.send.len(), 2, "message 2 and the presence");
    let mut replayed = Out::default();
    higher.s.receive(d.addr, hs1[3], &hs1[4..], now, &mut replayed);
    assert!(replayed.send.is_empty());
}

#[test]
fn game_events_arrive_once_through_losses() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let mut out = Out::default();
    assert!(a.s.send_game(2000, "{\"invite\":1}", now, &mut out));
    assert!(!a.s.send_game(7, "nobody", now, &mut out));
    // The first copy is lost; the next one arrives, its ack too.
    let lost = std::cell::Cell::new(0);
    let drop = |d: &[u8]| d[3] == session::DATA && lost.replace(lost.get() + 1) == 0;
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 12, &drop);
    let events: Vec<_> = b.s.events.drain(..).collect();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].from, events[0].name.as_str(), events[0].text.as_str()), (1000, "Ann", "{\"invite\":1}"));
    assert!(a.s.friends[&b.key()].outbox.is_empty());
}

#[test]
fn removing_a_friend_tells_it() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    let mut out = Out::default();
    a.s.remove_friend(&b.key(), &mut out);
    let from = a.addr;
    run(&mut [&mut a, &mut b], out, from, &mut now, 2, &keep);
    assert!(a.s.friends.is_empty() && b.s.friends.is_empty());
    assert!(a.restarted().s.friends.is_empty());
}

#[test]
fn friends_go_offline_when_silent() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    // Bob's datagrams stop.
    let mut out = Out::default();
    for _ in 0..(SILENCE.as_millis() / 250 + 4) {
        now += Duration::from_millis(250);
        a.s.tick(now, Vec::new(), &[], 1, &mut out);
    }
    assert!(!a.s.friends[&b.key()].connected());
}

#[test]
fn invites_come_as_notices_once() {
    let mut now = Instant::now();
    let (mut a, mut b) = (Player::new("Ann", 1), Player::new("Bob", 2));
    befriend(&mut a, &mut b, true, &mut now);
    b.s.notices.clear();
    // From a session for invited players only: its lobby and session go too.
    a.s.joinable = false;
    a.s.local.session = Some("SESSION".into());
    for _ in 0..2 {
        let mut out = Out::default();
        assert!(a.s.send_invite(2000, Some("LOBBY".into()), now, &mut out));
        let from = a.addr;
        run(&mut [&mut a, &mut b], out, from, &mut now, 2, &keep);
    }
    // The second one, so soon: not again.
    assert_eq!(b.s.notices.drain(..).collect::<Vec<_>>(), [Notice::Invite("Ann".into())]);
    let invited = b.s.friends[&a.key()].invited.as_ref().unwrap();
    assert_eq!((invited.lobby.as_deref(), invited.session.as_deref()), (Some("LOBBY"), Some("SESSION")));
    // Not a friend's id: nothing sent.
    assert!(!a.s.send_invite(7, None, now, &mut Out::default()));
}

#[test]
fn players_met_ask_each_other() {
    let mut now = Instant::now();
    let (mut a, mut c) = (Player::new("Ann", 1), Player::new("Carl", 3));
    let card = |p: &Player| crate::p2p::Card { gamer: p.s.local.id, key: p.key(), name: p.s.local.name.clone() };
    let (ca, cc) = (card(&a), card(&c));
    // They meet in a lobby: each one's card and address.
    let mut out = Out::default();
    a.s.meet(&[(cc.clone(), c.addr)], now, &mut out);
    c.s.meet(&[(ca.clone(), a.addr)], now, &mut out);
    assert_eq!(a.s.recent.len(), 1);
    // Ann asks Carl; Carl says yes.
    a.s.asked.push(Ask::new(crypto::hex(&c.key()), Some((c.key(), "Carl".into()))));
    let mut out = Out::default();
    a.s.meet(&[(cc, c.addr)], now, &mut out);
    let from = a.addr;
    run(&mut [&mut a, &mut c], out, from, &mut now, 4, &keep);
    assert_eq!(c.s.notices.drain(..).collect::<Vec<_>>(), [Notice::Request("Ann".into())]);
    let mut out = Out::default();
    (a.s.summarized, c.s.summarized) = (true, true);
    c.s.answer(&a.key(), true, now, &mut out);
    let from = c.addr;
    run(&mut [&mut a, &mut c], out, from, &mut now, 4, &keep);
    assert!(a.connected_to(&c) && c.connected_to(&a));
    // Said once: no "online" for a friend just made.
    assert_eq!(a.s.notices.drain(..).collect::<Vec<_>>(), [Notice::Accepted("Carl".into())]);
    assert!(c.s.notices.is_empty());
    // Someone not met (no card from that address): not answered.
    let mut d = Player::new("Dan", 4);
    let (h, first) = Handshake::friend(d.s.identity.as_ref().unwrap(), &c.key(), &unix_ms().to_be_bytes(), c.addr, now).unwrap();
    d.s.handshakes.insert(h.index, (h, Owner::Asking("x".into())));
    let mut answer = Out::default();
    c.s.receive(d.addr, first[3], &first[4..], now, &mut answer);
    assert!(answer.send.is_empty());
}
