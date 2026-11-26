//! Update check + self-update from the `latest-build` GitHub release.
//!
//! CI embeds the commit SHA (`RV_BUILD_SHA`); the release body contains
//! "Bản build tự động từ commit <sha>". A different SHA means a newer build.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_SHA: &str = match option_env!("RV_BUILD_SHA") {
    Some(s) => s,
    None => "dev",
};
pub const API_URL: &str = "https://api.github.com/repos/XclipseX/window-remote-view/releases/tags/latest-build";
pub const RELEASE_URL: &str = "https://github.com/XclipseX/window-remote-view/releases/tag/latest-build";
const ASSET_NAME: &str = "RemoteView.exe";
const MIN_EXE_SIZE: u64 = 1024 * 1024;
pub const OLD_NAME: &str = "RemoteView.old.exe";

pub fn short_sha() -> &'static str {
    &BUILD_SHA[..BUILD_SHA.len().min(7)]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub sha: String,
    pub download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    UpToDate,
    Available(UpdateInfo),
    /// Release page not reachable via the API (private repo -> 404).
    Inaccessible,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum UpdateMsg {
    Checked { manual: bool, result: CheckResult },
    Progress { done: u64, total: u64 },
    Downloaded(PathBuf),
    Failed(String),
}

/// First 7..40 hex chars after "commit " in the release body.
pub fn parse_sha(body: &str) -> Option<String> {
    let i = body.find("commit ")? + "commit ".len();
    let sha: String = body[i..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (sha.len() >= 7).then(|| sha.to_ascii_lowercase())
}

/// True if `remote` is a different commit than `local` (prefix-tolerant).
/// `dev` builds never report updates.
pub fn is_newer(local: &str, remote: &str) -> bool {
    if local == "dev" || local.is_empty() {
        return false;
    }
    let (l, r) = (local.to_ascii_lowercase(), remote.to_ascii_lowercase());
    !(l.starts_with(&r) || r.starts_with(&l))
}

pub fn evaluate(local: &str, release: &serde_json::Value) -> CheckResult {
    let Some(sha) = release["body"].as_str().and_then(parse_sha) else {
        return CheckResult::Failed("không đọc được mã commit trong bản phát hành".into());
    };
    if !is_newer(local, &sha) {
        return CheckResult::UpToDate;
    }
    let asset = release["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == ASSET_NAME));
    match asset.and_then(|a| Some((a["browser_download_url"].as_str()?.to_string(), a["size"].as_u64().unwrap_or(0)))) {
        Some((download_url, size)) => CheckResult::Available(UpdateInfo { sha, download_url, size }),
        None => CheckResult::Failed("bản phát hành không có RemoteView.exe".into()),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .user_agent("RemoteView-updater")
        .build()
        .into()
}

pub fn check() -> CheckResult {
    let resp = agent().get(API_URL).header("Accept", "application/vnd.github+json").call();
    match resp {
        Ok(mut r) => match r.body_mut().read_json::<serde_json::Value>() {
            Ok(v) => evaluate(BUILD_SHA, &v),
            Err(e) => CheckResult::Failed(e.to_string()),
        },
        Err(ureq::Error::StatusCode(404 | 401 | 403)) => CheckResult::Inaccessible,
        Err(e) => CheckResult::Failed(e.to_string()),
    }
}

pub fn spawn_check(tx: Sender<UpdateMsg>, manual: bool, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let result = check();
        let _ = tx.send(UpdateMsg::Checked { manual, result });
        ctx.request_repaint();
    });
}

/// `MZ` header and plausible size.
pub fn verify_exe(path: &Path) -> Result<(), String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len <= MIN_EXE_SIZE {
        return Err(format!("tệp tải về quá nhỏ ({len} byte)"));
    }
    let mut h = [0u8; 2];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut h)).map_err(|e| e.to_string())?;
    if &h != b"MZ" {
        return Err("tệp tải về không phải chương trình Windows (thiếu MZ)".into());
    }
    Ok(())
}

pub fn spawn_download(info: UpdateInfo, tx: Sender<UpdateMsg>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let msg = match download(&info, &tx, &ctx) {
            Ok(p) => UpdateMsg::Downloaded(p),
            Err(e) => UpdateMsg::Failed(e),
        };
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
}

fn download(info: &UpdateInfo, tx: &Sender<UpdateMsg>, ctx: &eframe::egui::Context) -> Result<PathBuf, String> {
    let dest = std::env::temp_dir().join("RemoteView-update.exe");
    let _ = std::fs::remove_file(&dest);
    let mut resp = agent().get(&info.download_url).call().map_err(|e| e.to_string())?;
    let total = resp.body().content_length().unwrap_or(info.size);
    let mut reader = resp.body_mut().as_reader();
    let mut f = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
    let (mut buf, mut done, mut last) = (vec![0u8; 64 * 1024], 0u64, 0u64);
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - last > 256 * 1024 {
            last = done;
            let _ = tx.send(UpdateMsg::Progress { done, total });
            ctx.request_repaint();
        }
    }
    f.flush().map_err(|e| e.to_string())?;
    drop(f);
    let _ = tx.send(UpdateMsg::Progress { done, total: done });
    if info.size > 0 && done != info.size {
        return Err(format!("tải thiếu dữ liệu ({done}/{} byte)", info.size));
    }
    verify_exe(&dest)?;
    Ok(dest)
}

/// Rename the running exe to `RemoteView.old.exe`, put `new_exe` in its place
/// and return the path to relaunch. Rolls back on failure.
pub fn replace_self(new_exe: &Path) -> Result<PathBuf, String> {
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    swap_files(&cur, new_exe)?;
    Ok(cur)
}

pub fn swap_files(cur: &Path, new_exe: &Path) -> Result<(), String> {
    let old = cur.with_file_name(OLD_NAME);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(cur, &old).map_err(|e| format!("không đổi tên được tệp đang chạy: {e}"))?;
    let placed = std::fs::rename(new_exe, cur).or_else(|_| std::fs::copy(new_exe, cur).map(|_| ()));
    if let Err(e) = placed {
        let _ = std::fs::rename(&old, cur);
        return Err(format!("không ghi được bản mới: {e}"));
    }
    let _ = std::fs::remove_file(new_exe);
    Ok(())
}

pub fn relaunch(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Delete the leftover `RemoteView.old.exe` from a previous self-update.
pub fn cleanup_old() {
    if let Ok(cur) = std::env::current_exe() {
        let _ = std::fs::remove_file(cur.with_file_name(OLD_NAME));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha_parse_and_compare() {
        let body = "Bản build tự động từ commit 68A502A1234567890abcdef.\n- RemoteView.apk";
        let sha = parse_sha(body).unwrap();
        assert_eq!(sha, "68a502a1234567890abcdef");
        assert!(!is_newer("68a502a", &sha));
        assert!(is_newer("1111111", &sha));
        assert!(!is_newer("dev", &sha));
        assert!(parse_sha("no sha here").is_none());
    }

    #[test]
    fn evaluate_release() {
        let rel = json!({
            "body": "Bản build tự động từ commit abcdef1234.",
            "assets": [{"name": "RemoteView.exe", "browser_download_url": "https://x/RemoteView.exe", "size": 5000000}]
        });
        assert_eq!(evaluate("abcdef1", &rel), CheckResult::UpToDate);
        match evaluate("0000000", &rel) {
            CheckResult::Available(i) => assert_eq!(i.download_url, "https://x/RemoteView.exe"),
            o => panic!("{o:?}"),
        }
        assert!(matches!(evaluate("0000000", &json!({"body": "x", "assets": []})), CheckResult::Failed(_)));
    }

    #[test]
    fn verify_and_swap() {
        let dir = std::env::temp_dir().join(format!("rv-upd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (cur, new) = (dir.join("RemoteView.exe"), dir.join("new.exe"));
        std::fs::write(&cur, b"old").unwrap();
        let mut good = vec![0u8; 2 * 1024 * 1024];
        good[..2].copy_from_slice(b"MZ");
        std::fs::write(&new, &good).unwrap();
        assert!(verify_exe(&new).is_ok());
        std::fs::write(dir.join("bad.exe"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
        assert!(verify_exe(&dir.join("bad.exe")).is_err());
        std::fs::write(dir.join("small.exe"), b"MZ").unwrap();
        assert!(verify_exe(&dir.join("small.exe")).is_err());
        swap_files(&cur, &new).unwrap();
        assert_eq!(std::fs::metadata(&cur).unwrap().len(), good.len() as u64);
        assert_eq!(std::fs::read(dir.join(OLD_NAME)).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
