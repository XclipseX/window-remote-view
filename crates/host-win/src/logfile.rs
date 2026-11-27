//! Log files: `%LOCALAPPDATA%\RemoteView\logs\remoteview-<date-time>.log`
//! (rotated, newest [`KEEP`] kept) and reassembly of logs uploaded by the
//! Android client (`ControlMessage::LogChunk`) as `android-<name>.log`.
//!
//! Everything but the clock is plain `std`, so it is unit-tested on any OS.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};
#[cfg(not(windows))]
use std::time::UNIX_EPOCH;

/// How many `remoteview-*.log` files (and `android-*.log` files) are kept.
pub const KEEP: usize = 10;
/// Largest Android log accepted (the sender keeps the tail when it is bigger).
pub const MAX_ANDROID_LOG: usize = 8 * 1024 * 1024;
/// Prefix of the GUI/host log files.
pub const PREFIX: &str = "remoteview";

static CURRENT: Mutex<Option<(PathBuf, File)>> = Mutex::new(None);

/// Calendar time (local on Windows, UTC elsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

/// Days since 1970-01-01 -> (year, month, day) in the proleptic Gregorian calendar.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// UTC breakdown of a unix timestamp.
pub fn utc_from_unix(secs: i64, millis: u32) -> DateTime {
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400) as u32;
    DateTime { year, month, day, hour: s / 3600, minute: s % 3600 / 60, second: s % 60, millis }
}

/// Current time: local time on Windows, UTC elsewhere.
pub fn now() -> DateTime {
    #[cfg(windows)]
    unsafe {
        let t = windows::Win32::System::SystemInformation::GetLocalTime();
        return DateTime {
            year: i64::from(t.wYear),
            month: u32::from(t.wMonth),
            day: u32::from(t.wDay),
            hour: u32::from(t.wHour),
            minute: u32::from(t.wMinute),
            second: u32::from(t.wSecond),
            millis: u32::from(t.wMilliseconds),
        };
    }
    #[cfg(not(windows))]
    {
        let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        utc_from_unix(d.as_secs() as i64, d.subsec_millis())
    }
}

impl DateTime {
    /// `20261126-143005`, sorts chronologically.
    pub fn file_stamp(&self) -> String {
        format!("{:04}{:02}{:02}-{:02}{:02}{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
    /// `2026-11-26 14:30:05.123`
    pub fn line_stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }
}

/// `%LOCALAPPDATA%\RemoteView\logs` (XDG data dir / temp dir off Windows).
pub fn logs_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("RemoteView").join("logs")
}

fn is_log_of(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|rest| rest.as_bytes().first().is_some_and(u8::is_ascii_digit) && rest.ends_with(".log"))
}

/// `remoteview-*.log` files in `dir`, newest first (names embed the timestamp).
pub fn list_logs(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| is_log_of(n, prefix)))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.reverse();
    v
}

/// Deletes the oldest `prefix-*.log` files so that at most `keep` remain.
pub fn rotate(dir: &Path, prefix: &str, keep: usize) {
    for old in list_logs(dir, prefix).into_iter().skip(keep) {
        let _ = fs::remove_file(old);
    }
}

fn rotate_android(dir: &Path, keep: usize) {
    let mut v: Vec<(SystemTime, PathBuf)> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    n.starts_with("android-") && n.ends_with(".log")
                })
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (_, p) in v.into_iter().skip(keep) {
        let _ = fs::remove_file(p);
    }
}

/// Creates `remoteview-<date-time>.log` (after rotating old ones) and routes
/// [`write_line`] to it. Returns the path, or `None` when no log directory is
/// writable (logging then silently stays on stderr / the GUI).
pub fn init(prefix: &str) -> Option<PathBuf> {
    let dir = logs_dir();
    fs::create_dir_all(&dir).ok()?;
    rotate(&dir, prefix, KEEP.saturating_sub(1));
    let mut path = dir.join(format!("{prefix}-{}.log", now().file_stamp()));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{prefix}-{}-{n}.log", now().file_stamp()));
        n += 1;
    }
    let file = OpenOptions::new().create(true).append(true).open(&path).ok()?;
    *CURRENT.lock().ok()? = Some((path.clone(), file));
    Some(path)
}

/// Path of the log file opened by [`init`].
pub fn current_path() -> Option<PathBuf> {
    CURRENT.lock().ok()?.as_ref().map(|(p, _)| p.clone())
}

/// Appends one timestamped line (no-op before [`init`]).
pub fn write_line(level: &str, msg: &str) {
    if let Ok(mut g) = CURRENT.lock() {
        if let Some((_, f)) = g.as_mut() {
            let _ = writeln!(f, "{} [{level}] {msg}", now().line_stamp());
            let _ = f.flush();
        }
    }
}

/// Files for the "export logs" zip: newest 2 GUI logs, newest 2 standalone host logs,
/// newest 2 service logs, plus up to 4 newest received Android logs.
pub fn export_set(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = list_logs(dir, PREFIX).into_iter().take(2).collect();
    v.extend(list_logs(dir, "remoteview-host").into_iter().take(2));
    v.extend(list_logs(dir, "remoteview-service").into_iter().take(2));
    let mut android: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("android-") && n.ends_with(".log"))
                })
                .collect()
        })
        .unwrap_or_default();
    android.sort();
    android.reverse();
    v.extend(android.into_iter().take(4));
    v
}

/// Reduces an uploaded log name to a safe file stem (no path parts, no
/// `android-` prefix / `.log` suffix, ASCII `[A-Za-z0-9._-]` only).
pub fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let base = base.strip_prefix("android-").unwrap_or(base);
    let base = base.strip_suffix(".log").unwrap_or(base);
    let s: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .take(48)
        .collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() {
        "device".into()
    } else {
        s
    }
}

/// Writes a received Android log as `android-<name>.log` in `dir`.
pub fn save_android_log(dir: &Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("android-{}.log", sanitize_name(name)));
    fs::write(&path, data)?;
    rotate_android(dir, KEEP);
    Ok(path)
}

/// A fully (or, after a timeout, partially) received log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedLog {
    pub name: String,
    pub data: Vec<u8>,
    /// Chunks that never arrived (datagram loss); a marker line is appended.
    pub missing: u32,
}

struct Partial {
    chunks: BTreeMap<u32, Vec<u8>>,
    last_seq: Option<u32>,
    bytes: usize,
    touched: Instant,
}

impl Partial {
    fn finish(self, name: String) -> FinishedLog {
        let top = self.last_seq.or_else(|| self.chunks.keys().next_back().copied()).map_or(0, |s| u64::from(s) + 1);
        let missing = top.saturating_sub(self.chunks.len() as u64) as u32;
        let mut data = Vec::with_capacity(self.bytes + 64);
        for c in self.chunks.into_values() {
            data.extend_from_slice(&c);
        }
        if missing > 0 {
            data.extend_from_slice(format!("\n--- thiếu {missing} đoạn log (mất gói) ---\n").as_bytes());
        }
        FinishedLog { name, data, missing }
    }
}

/// Reassembles `LogChunk`s (any arrival order, tolerant of lost chunks).
#[derive(Default)]
pub struct LogAssembler {
    files: HashMap<String, Partial>,
}

/// Concurrent uploads kept; the oldest is dropped beyond this.
const MAX_PARTIALS: usize = 4;

impl LogAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one chunk; returns the log when it just became complete.
    pub fn push(&mut self, name: &str, seq: u32, last: bool, data: &[u8], now: Instant) -> Option<FinishedLog> {
        if seq == 0 && self.files.get(name).is_some_and(|p| p.chunks.contains_key(&0)) {
            // A new upload of the same file replaces a stale partial one.
            self.files.remove(name);
        }
        if !self.files.contains_key(name) && self.files.len() >= MAX_PARTIALS {
            if let Some(oldest) = self.files.iter().min_by_key(|(_, p)| p.touched).map(|(k, _)| k.clone()) {
                self.files.remove(&oldest);
            }
        }
        let p = self
            .files
            .entry(name.to_string())
            .or_insert_with(|| Partial { chunks: BTreeMap::new(), last_seq: None, bytes: 0, touched: now });
        p.touched = now;
        if last {
            p.last_seq = Some(seq);
        }
        if p.bytes + data.len() > MAX_ANDROID_LOG {
            self.files.remove(name);
            return None;
        }
        if p.chunks.insert(seq, data.to_vec()).is_none() {
            p.bytes += data.len();
        }
        let complete = p.last_seq.is_some_and(|l| p.chunks.len() as u64 == u64::from(l) + 1);
        if complete {
            let p = self.files.remove(name)?;
            return Some(p.finish(name.to_string()));
        }
        None
    }

    /// Finishes uploads that have been silent for `idle` (their tail was lost).
    pub fn flush_stale(&mut self, now: Instant, idle: Duration) -> Vec<FinishedLog> {
        let stale: Vec<String> = self
            .files
            .iter()
            .filter(|(_, p)| now.saturating_duration_since(p.touched) >= idle)
            .map(|(k, _)| k.clone())
            .collect();
        stale.into_iter().filter_map(|k| self.files.remove(&k).map(|p| p.finish(k))).collect()
    }

    pub fn pending(&self) -> usize {
        self.files.len()
    }
}

/// Rate-limits repeated warnings: logs only when the message differs from the
/// previous one, counting what was suppressed in between.
#[derive(Default)]
pub struct WarnGate {
    last: Option<String>,
    suppressed: u32,
}

impl WarnGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` when `msg` should be logged now (first time or changed).
    pub fn should_log(&mut self, msg: &str) -> bool {
        if self.last.as_deref() == Some(msg) {
            self.suppressed = self.suppressed.saturating_add(1);
            false
        } else {
            self.last = Some(msg.to_string());
            self.suppressed = 0;
            true
        }
    }

    /// Call when the failing operation succeeds again: returns how many
    /// repeats were suppressed (if a failure was active) and re-arms the gate.
    pub fn recovered(&mut self) -> Option<u32> {
        self.last.take()?;
        Some(std::mem::take(&mut self.suppressed))
    }

    pub fn active(&self) -> bool {
        self.last.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        let t = utc_from_unix(1_700_000_000, 7); // 2023-11-14 22:13:20 UTC
        assert_eq!(t.line_stamp(), "2023-11-14 22:13:20.007");
        assert_eq!(t.file_stamp(), "20231114-221320");
    }

    #[test]
    fn rotation_keeps_newest() {
        let dir = std::env::temp_dir().join(format!("rv-logs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for i in 0..14 {
            fs::write(dir.join(format!("remoteview-20260101-0000{i:02}.log")), b"x").unwrap();
        }
        fs::write(dir.join("android-tab.log"), b"a").unwrap();
        fs::write(dir.join("other.txt"), b"o").unwrap();
        rotate(&dir, PREFIX, 9);
        let left = list_logs(&dir, PREFIX);
        assert_eq!(left.len(), 9);
        assert!(left[0].to_string_lossy().ends_with("000013.log"));
        assert!(dir.join("android-tab.log").exists() && dir.join("other.txt").exists());
        let set = export_set(&dir);
        assert_eq!(set.len(), 3); // current + previous + android
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_sanitised() {
        assert_eq!(sanitize_name("android-Tab S10+.log"), "Tab_S10_");
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name("..\\..\\x.log"), "x");
        assert_eq!(sanitize_name(""), "device");
        assert_eq!(sanitize_name("...."), "device");
        assert_eq!(sanitize_name(&"a".repeat(64)).len(), 48);
    }

    #[test]
    fn assembles_in_order_and_out_of_order() {
        let t = Instant::now();
        let mut a = LogAssembler::new();
        assert!(a.push("x", 0, false, b"he", t).is_none());
        assert!(a.push("x", 1, false, b"llo ", t).is_none());
        let done = a.push("x", 2, true, b"world", t).unwrap();
        assert_eq!((done.data.as_slice(), done.missing), (&b"hello world"[..], 0));
        assert_eq!(a.pending(), 0);
        // last chunk first
        assert!(a.push("y", 2, true, b"c", t).is_none());
        assert!(a.push("y", 0, false, b"a", t).is_none());
        let done = a.push("y", 1, false, b"b", t).unwrap();
        assert_eq!(done.data, b"abc");
    }

    #[test]
    fn lost_chunks_are_flushed_with_marker() {
        let t = Instant::now();
        let mut a = LogAssembler::new();
        a.push("z", 0, false, b"one\n", t);
        assert!(a.push("z", 3, true, b"four\n", t).is_none());
        assert!(a.flush_stale(t + Duration::from_secs(1), Duration::from_secs(3)).is_empty());
        let v = a.flush_stale(t + Duration::from_secs(4), Duration::from_secs(3));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].missing, 2);
        assert!(String::from_utf8_lossy(&v[0].data).contains("thiếu 2 đoạn"));
        assert_eq!(a.pending(), 0);
    }

    #[test]
    fn restart_and_limits() {
        let t = Instant::now();
        let mut a = LogAssembler::new();
        a.push("r", 0, false, b"old", t);
        a.push("r", 0, false, b"new", t); // restart
        assert_eq!(a.push("r", 1, true, b"!", t).unwrap().data, b"new!");
        for i in 0..6 {
            a.push(&format!("f{i}"), 0, false, b"x", t + Duration::from_millis(i));
        }
        assert_eq!(a.pending(), MAX_PARTIALS);
        let big = vec![0u8; 1024 * 1024];
        for s in 0..9 {
            a.push("big", s, false, &big, t);
        }
        assert_eq!(a.pending(), MAX_PARTIALS - 1); // "big" evicted an old one, then was dropped for exceeding the cap
    }

    #[test]
    fn save_android_log_file() {
        let dir = std::env::temp_dir().join(format!("rv-android-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let p = save_android_log(&dir, "../Tab S10.log", b"hello").unwrap();
        assert_eq!(p.file_name().unwrap(), "android-Tab_S10.log");
        assert_eq!(fs::read(&p).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn warn_gate_logs_once_per_change() {
        let mut g = WarnGate::new();
        assert!(g.should_log("denied"));
        assert!(!g.should_log("denied"));
        assert!(!g.should_log("denied"));
        assert!(g.should_log("other"));
        assert!(g.active());
        assert_eq!(g.recovered(), Some(0));
        assert_eq!(g.recovered(), None);
        assert!(g.should_log("denied"));
        assert!(!g.should_log("denied"));
        assert_eq!(g.recovered(), Some(1));
    }

    #[test]
    fn export_set_keeps_gui_logs_when_service_logs_exist() {
        let dir = std::env::temp_dir().join(format!("rv-export-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("remoteview-20261127-144810.log"), b"gui").unwrap();
        fs::write(dir.join("remoteview-service-20261127-145500.log"), b"svc1").unwrap();
        fs::write(dir.join("remoteview-service-20261127-145600.log"), b"svc2").unwrap();
        fs::write(dir.join("android-sm-x826b-20261127-145645.log"), b"and").unwrap();
        let names: Vec<String> = export_set(&dir)
            .into_iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert!(names.contains(&"remoteview-20261127-144810.log".to_string()), "GUI log missing: {names:?}");
        assert!(names.contains(&"remoteview-service-20261127-145600.log".to_string()), "service log missing: {names:?}");
        assert!(names.contains(&"android-sm-x826b-20261127-145645.log".to_string()), "android log missing: {names:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
