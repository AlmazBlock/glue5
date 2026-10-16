//! Servers on the local network, by Bonjour (mDNS service `_glue5._tcp`).
//!
//! Apple systems go through the system's responder (`dns_sd`): it needs no
//! multicast entitlement on iOS and tvOS, only the service in the app's
//! `NSBonjourServices`; a Mac sharing its game folder announces it the same
//! way. Elsewhere [`find`] sends one mDNS query from an ordinary UDP port,
//! which responders answer by unicast (RFC 6762, 6.7): no multicast
//! reception, so no Android multicast lock. Servers on other systems are not
//! announced: their address is typed in.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

pub const SERVICE: &str = "_glue5._tcp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The server's name (its computer's).
    pub name: String,
    /// What to connect to: `host:port`.
    pub address: String,
}

/// The servers that answer within `timeout`.
pub fn find(timeout: Duration) -> Vec<Found> {
    #[cfg(target_vendor = "apple")]
    return apple::find(timeout);
    #[cfg(not(target_vendor = "apple"))]
    return query(timeout).unwrap_or_default();
}

/// Announces a server on `port` until dropped (Apple systems).
pub struct Advert {
    #[cfg(target_vendor = "apple")]
    _sd: apple::Ref,
}

pub fn advertise(port: u16) -> Option<Advert> {
    advertise_as(None, port)
}

/// [`advertise`] under `name` (the computer's name by default).
fn advertise_as(name: Option<&str>, port: u16) -> Option<Advert> {
    #[cfg(target_vendor = "apple")]
    return apple::register(name, port).map(|sd| Advert { _sd: sd });
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = (name, port);
        None
    }
}

#[cfg(target_vendor = "apple")]
mod apple {
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::time::{Duration, Instant};

    use super::{Found, SERVICE};

    type SdRef = *mut c_void;
    type BrowseReply =
        extern "C" fn(SdRef, u32, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);
    type ResolveReply =
        extern "C" fn(SdRef, u32, u32, i32, *const c_char, *const c_char, u16, u16, *const u8, *mut c_void);
    type RegisterReply = extern "C" fn(SdRef, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);

    // libSystem's dns_sd.
    unsafe extern "C" {
        fn DNSServiceRegister(
            sd: *mut SdRef,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port: u16,
            txt_len: u16,
            txt: *const c_void,
            reply: Option<RegisterReply>,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceBrowse(
            sd: *mut SdRef,
            flags: u32,
            interface: u32,
            regtype: *const c_char,
            domain: *const c_char,
            reply: BrowseReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceResolve(
            sd: *mut SdRef,
            flags: u32,
            interface: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            reply: ResolveReply,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefSockFD(sd: SdRef) -> i32;
        fn DNSServiceProcessResult(sd: SdRef) -> i32;
        fn DNSServiceRefDeallocate(sd: SdRef);
    }

    const ADD: u32 = 0x2;

    /// A `DNSServiceRef`, deallocated when dropped.
    pub struct Ref(SdRef);

    unsafe impl Send for Ref {}
    unsafe impl Sync for Ref {}

    impl Drop for Ref {
        fn drop(&mut self) {
            unsafe { DNSServiceRefDeallocate(self.0) }
        }
    }

    pub fn register(name: Option<&str>, port: u16) -> Option<Ref> {
        let regtype = CString::new(SERVICE).unwrap();
        let name = name.and_then(|n| CString::new(n).ok());
        let mut sd: SdRef = std::ptr::null_mut();
        let err = unsafe {
            DNSServiceRegister(
                &mut sd,
                0,
                0,
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                regtype.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                port.to_be(),
                0,
                std::ptr::null(),
                None,
                std::ptr::null_mut(),
            )
        };
        if err != 0 {
            log::warn!("bonjour: no announcement (error {err})");
            return None;
        }
        Some(Ref(sd))
    }

    /// Processes `sd`'s answers until `until`, or until `done` says so.
    fn pump(sd: SdRef, until: Instant, done: &dyn Fn() -> bool) {
        let fd = unsafe { DNSServiceRefSockFD(sd) };
        while !done() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let ready = unsafe { libc::poll(&mut p, 1, left.as_millis().max(1) as i32) };
            if ready <= 0 || unsafe { DNSServiceProcessResult(sd) } != 0 {
                return;
            }
        }
    }

    struct Service {
        name: String,
        regtype: CString,
        domain: CString,
        interface: u32,
    }

    extern "C" fn browsed(
        _: SdRef,
        flags: u32,
        interface: u32,
        err: i32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        context: *mut c_void,
    ) {
        if err != 0 || flags & ADD == 0 {
            return;
        }
        let found = unsafe { &mut *(context as *mut Vec<Service>) };
        let s = |p: *const c_char| unsafe { CStr::from_ptr(p) };
        let name = s(name).to_string_lossy().into_owned();
        if found.iter().any(|f| f.name == name) {
            return;
        }
        found.push(Service { name, regtype: s(regtype).into(), domain: s(domain).into(), interface });
    }

    extern "C" fn resolved(
        _: SdRef,
        _: u32,
        _: u32,
        err: i32,
        _: *const c_char,
        host: *const c_char,
        port: u16,
        _: u16,
        _: *const u8,
        context: *mut c_void,
    ) {
        if err != 0 {
            return;
        }
        let out = unsafe { &mut *(context as *mut Option<String>) };
        let host = unsafe { CStr::from_ptr(host) }.to_string_lossy().trim_end_matches('.').to_string();
        *out = Some(format!("{host}:{}", u16::from_be(port)));
    }

    pub fn find(timeout: Duration) -> Vec<Found> {
        let until = Instant::now() + timeout;
        let regtype = CString::new(SERVICE).unwrap();
        let mut services: Vec<Service> = Vec::new();
        let mut sd: SdRef = std::ptr::null_mut();
        let ctx = &mut services as *mut Vec<Service> as *mut c_void;
        if unsafe { DNSServiceBrowse(&mut sd, 0, 0, regtype.as_ptr(), std::ptr::null(), browsed, ctx) } != 0 {
            return Vec::new();
        }
        let browse = Ref(sd);
        // Most answers come at once; the rest of the time goes to resolving.
        pump(browse.0, Instant::now() + timeout / 2, &|| false);
        drop(browse);
        let mut found = Vec::new();
        for s in &services {
            let name = CString::new(s.name.clone()).unwrap_or_default();
            let mut address: Option<String> = None;
            let ctx = &mut address as *mut Option<String> as *mut c_void;
            let mut sd: SdRef = std::ptr::null_mut();
            let err = unsafe {
                DNSServiceResolve(&mut sd, 0, s.interface, name.as_ptr(), s.regtype.as_ptr(), s.domain.as_ptr(), resolved, ctx)
            };
            if err != 0 {
                continue;
            }
            let resolve = Ref(sd);
            let deadline = until.max(Instant::now() + Duration::from_millis(500));
            pump(resolve.0, deadline, &|| unsafe { (*(ctx as *const Option<String>)).is_some() });
            drop(resolve);
            if let Some(address) = address
                && !found.iter().any(|f: &Found| f.address == address)
            {
                found.push(Found { name: s.name.clone(), address });
            }
        }
        found
    }
}

/// One mDNS question for the service from an ordinary port; the answers
/// (pointer, service and address records) come back by unicast.
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
fn query(timeout: Duration) -> std::io::Result<Vec<Found>> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    let _ = socket.set_multicast_ttl_v4(255);
    let mut msg = vec![0x47, 0x35, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    put_name(&mut msg, &format!("{SERVICE}.local"));
    msg.extend_from_slice(&[0, 12, 0, 1]);
    socket.send_to(&msg, SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353))?;
    let until = Instant::now() + timeout;
    let (mut ptrs, mut srvs, mut addrs) = (Vec::new(), Vec::new(), Vec::new());
    let mut buf = [0u8; 9000];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        socket.set_read_timeout(Some(left))?;
        let Ok((n, from)) = socket.recv_from(&mut buf) else { break };
        parse(&buf[..n], from, &mut ptrs, &mut srvs, &mut addrs);
    }
    let mut found = Vec::new();
    for instance in ptrs {
        let Some((_, port, target)) = srvs.iter().find(|(i, _, _)| *i == instance) else { continue };
        let host = addrs.iter().find(|(h, _)| h == target).map_or_else(|| target.clone(), |(_, ip)| ip.to_string());
        let name = instance.split('.').next().unwrap_or(&instance).to_string();
        let address = format!("{host}:{port}");
        if !found.iter().any(|f: &Found| f.address == address) {
            found.push(Found { name, address });
        }
    }
    Ok(found)
}

fn put_name(msg: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        msg.push(label.len() as u8);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0);
}

/// A name at `at` (with compression), and the offset after it.
fn read_name(msg: &[u8], mut at: usize) -> Option<(String, usize)> {
    let (mut parts, mut end, mut jumps) = (Vec::new(), None, 0);
    loop {
        let len = *msg.get(at)? as usize;
        if len & 0xc0 == 0xc0 {
            let ptr = (len & 0x3f) << 8 | *msg.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            jumps += 1;
            if jumps > 32 {
                return None;
            }
            at = ptr;
        } else if len == 0 {
            return Some((parts.join("."), end.unwrap_or(at + 1)));
        } else {
            parts.push(String::from_utf8_lossy(msg.get(at + 1..at + 1 + len)?).into_owned());
            at += 1 + len;
        }
    }
}

type Srv = (String, u16, String);

fn parse(msg: &[u8], from: SocketAddr, ptrs: &mut Vec<String>, srvs: &mut Vec<Srv>, addrs: &mut Vec<(String, Ipv4Addr)>) {
    let count = |i: usize| msg.get(i..i + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let (questions, records) = (count(4), count(6) + count(8) + count(10));
    let mut at = 12;
    for _ in 0..questions {
        let Some((_, next)) = read_name(msg, at) else { return };
        at = next + 4;
    }
    for _ in 0..records {
        let Some((name, next)) = read_name(msg, at) else { return };
        let Some(head) = msg.get(next..next + 10) else { return };
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[8], head[9]]) as usize;
        let data = next + 10;
        let Some(rdata) = msg.get(data..data + len) else { return };
        let service = format!("{SERVICE}.local").to_ascii_lowercase();
        match kind {
            12 if name.eq_ignore_ascii_case(&service) => {
                if let Some((instance, _)) = read_name(msg, data)
                    && !ptrs.contains(&instance)
                {
                    ptrs.push(instance);
                }
            }
            33 if rdata.len() >= 7 => {
                let port = u16::from_be_bytes([rdata[4], rdata[5]]);
                if let Some((target, _)) = read_name(msg, data + 6) {
                    srvs.push((name, port, target));
                }
            }
            1 if rdata.len() == 4 => addrs.push((name, Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]))),
            _ => {}
        }
        at = data + len;
    }
    // A responder that sent only a pointer: its sender is the server.
    if srvs.is_empty() && !ptrs.is_empty() && let SocketAddr::V4(a) = from {
        for p in ptrs.iter() {
            srvs.push((p.clone(), crate::PORT, a.ip().to_string()));
        }
    }
}

#[test]
fn names() {
    let mut msg = vec![0u8; 12];
    put_name(&mut msg, "_glue5._tcp.local");
    let at = msg.len();
    msg.extend_from_slice(&[3, b'M', b'a', b'c', 0xc0, 12]);
    assert_eq!(read_name(&msg, 12).unwrap().0, "_glue5._tcp.local");
    assert_eq!(read_name(&msg, at).unwrap(), ("Mac._glue5._tcp.local".to_string(), at + 6));
}

#[cfg(target_vendor = "apple")]
#[test]
fn finds_an_announced_server() {
    let port = 47_199;
    // Its own name: another server on the network has the computer's.
    let _advert = advertise_as(Some(&format!("glue5 test {}", std::process::id())), port).expect("announced");
    let found = find(Duration::from_secs(3));
    assert!(found.iter().any(|f| f.address.ends_with(&format!(":{port}"))), "found {found:?}");
    // The query other systems send, answered by this Mac's responder.
    let found = query(Duration::from_secs(2)).unwrap();
    assert!(found.iter().any(|f| f.address.ends_with(&format!(":{port}"))), "the query found {found:?}");
}
