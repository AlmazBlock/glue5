//! Clocks and time zones, as the browser provides them.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::memory::Mem;

/// `performance.timeOrigin + performance.now()`: epoch milliseconds that never
/// go backwards.
pub(crate) struct Clock {
    origin_ms: f64,
    start: Instant,
}

impl Clock {
    pub fn new() -> Self {
        Clock { origin_ms: date_now(), start: Instant::now() }
    }

    pub fn now_ms(&self) -> f64 {
        self.origin_ms + self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// `Date.now()`.
pub(crate) fn date_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

// struct tm (wasm64): nine i32 fields, then tm_gmtoff (i64) at 40.
const TM_SEC: u64 = 0;
const TM_MIN: u64 = 4;
const TM_HOUR: u64 = 8;
const TM_MDAY: u64 = 12;
const TM_MON: u64 = 16;
const TM_YEAR: u64 = 20;
const TM_WDAY: u64 = 24;
const TM_YDAY: u64 = 28;
const TM_ISDST: u64 = 32;
const TM_GMTOFF: u64 = 40;

#[cfg(unix)]
fn host_tm(t: i64, local: bool) -> Option<libc::tm> {
    let t = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { if local { libc::localtime_r(&t, &mut tm) } else { libc::gmtime_r(&t, &mut tm) } };
    (!ok.is_null()).then_some(tm)
}

#[cfg(unix)]
#[allow(clippy::useless_conversion)] // tm_gmtoff is a C long: not i64 everywhere
fn write_tm(mem: Mem, at: u64, tm: &libc::tm, local: bool) {
    mem.set_i32(at + TM_SEC, tm.tm_sec);
    mem.set_i32(at + TM_MIN, tm.tm_min);
    mem.set_i32(at + TM_HOUR, tm.tm_hour);
    mem.set_i32(at + TM_MDAY, tm.tm_mday);
    mem.set_i32(at + TM_MON, tm.tm_mon);
    mem.set_i32(at + TM_YEAR, tm.tm_year);
    mem.set_i32(at + TM_WDAY, tm.tm_wday);
    mem.set_i32(at + TM_YDAY, tm.tm_yday);
    if local {
        mem.set_i32(at + TM_ISDST, tm.tm_isdst.max(0));
        mem.set_i64(at + TM_GMTOFF, i64::from(tm.tm_gmtoff));
    }
}

/// `_localtime_js` / `_gmtime_js`.
#[cfg(unix)]
pub(crate) fn tm_from_time(mem: Mem, t: i64, at: u64, local: bool) -> u32 {
    match host_tm(t, local) {
        Some(tm) => {
            write_tm(mem, at, &tm, local);
            0
        }
        None => 1,
    }
}

/// `_mktime_js`: normalises the struct in place, returns the time (or -1).
#[cfg(unix)]
pub(crate) fn mktime(mem: Mem, at: u64) -> i64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_sec = mem.i32(at + TM_SEC);
    tm.tm_min = mem.i32(at + TM_MIN);
    tm.tm_hour = mem.i32(at + TM_HOUR);
    tm.tm_mday = mem.i32(at + TM_MDAY);
    tm.tm_mon = mem.i32(at + TM_MON);
    tm.tm_year = mem.i32(at + TM_YEAR);
    tm.tm_isdst = mem.i32(at + TM_ISDST);
    let t = unsafe { libc::mktime(&mut tm) };
    if t == -1 {
        return -1;
    }
    write_tm(mem, at, &tm, true);
    t as i64
}

/// UTC offset in minutes, JS style (positive west of UTC), at time `t`.
#[cfg(unix)]
#[allow(clippy::useless_conversion)] // tm_gmtoff is a C long: not i64 everywhere
fn js_offset(t: i64) -> i64 {
    host_tm(t, true).map_or(0, |tm| -i64::from(tm.tm_gmtoff) / 60)
}

/// `_tzset_js(timezone, daylight, std_name, dst_name)`.
#[cfg(unix)]
pub(crate) fn tzset(mem: Mem, timezone: u64, daylight: u64, std_name: u64, dst_name: u64) {
    let now = (date_now() / 1000.0) as i64;
    let year = host_tm(now, true).map_or(70, |tm| tm.tm_year);
    let at = |mon: i32| {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = year;
        tm.tm_mon = mon;
        tm.tm_mday = 1;
        tm.tm_isdst = -1;
        unsafe { libc::mktime(&mut tm) as i64 }
    };
    let (winter, summer) = (js_offset(at(0)), js_offset(at(6)));
    mem.set_u64(timezone, (winter.max(summer) * 60) as u64);
    mem.set_i32(daylight, (winter != summer) as i32);
    let zone = |off: i64| format!("UTC{}{:02}{:02}", if off >= 0 { '-' } else { '+' }, off.abs() / 60, off.abs() % 60);
    let (w, s) = (zone(winter), zone(summer));
    if summer < winter {
        mem.write_str(std_name, &w, 17);
        mem.write_str(dst_name, &s, 17);
    } else {
        mem.write_str(dst_name, &w, 17);
        mem.write_str(std_name, &s, 17);
    }
}
