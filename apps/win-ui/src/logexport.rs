//! "Xuất log": zips the current + previous log (and any Android logs received
//! over the session) to `Desktop\RemoteView-logs-<time>.zip` and reveals it in Explorer.

use std::io::Write;
use std::path::{Path, PathBuf};

use rv_host_win::logfile;

pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    !data.iter().fold(!0u32, |c, &b| table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8))
}

fn dos_time_date(t: &logfile::DateTime) -> (u16, u16) {
    let time = ((t.hour << 11) | (t.minute << 5) | (t.second / 2)) as u16;
    let year = (t.year.clamp(1980, 2107) - 1980) as u32;
    let date = ((year << 9) | (t.month << 5) | t.day) as u16;
    (time, date)
}

/// Builds a ZIP archive (entries stored uncompressed, UTF-8 names) in memory.
pub fn zip_bytes(files: &[(String, Vec<u8>)], stamp: &logfile::DateTime) -> Vec<u8> {
    let (dt, dd) = dos_time_date(stamp);
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let nb = name.as_bytes();
        // local file header
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&dt.to_le_bytes());
        out.extend_from_slice(&dd.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(nb);
        out.extend_from_slice(data);
        // central directory entry
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&dt.to_le_bytes());
        central.extend_from_slice(&dd.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, int attr, ext attr
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(nb);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // disk numbers
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Where the zip goes: the Desktop (home dir if there is none).
fn target_dir() -> Option<PathBuf> {
    dirs::desktop_dir().filter(|d| d.is_dir()).or_else(dirs::home_dir)
}

/// Collects the logs from `dirs` (deduplicating filenames) and writes the zip into `dest_dir`.
pub fn export_from_dirs(dirs: &[PathBuf], dest_dir: &Path) -> Result<PathBuf, String> {
    let now = logfile::now();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    let mut info = rv_host_win::sysinfo::header_lines("RemoteView").join("\r\n");
    info.push_str("\r\n");
    entries.push(("info.txt".into(), info.into_bytes()));
    for dir in dirs {
        for p in logfile::export_set(dir) {
            // The live log is open for append; a read error just skips that file.
            if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                if seen.insert(n.to_string()) {
                    if let Ok(d) = std::fs::read(&p) {
                        entries.push((n.to_string(), d));
                    }
                }
            }
        }
    }
    if entries.len() == 1 {
        let first = dirs.first().map(|d| d.display().to_string()).unwrap_or_default();
        return Err(format!("Chưa có file log nào trong {first}"));
    }
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let path = dest_dir.join(format!("RemoteView-logs-{}.zip", now.file_stamp()));
    let mut f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(&zip_bytes(&entries, &now)).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Collects the logs from `dir` and writes the zip into `dest_dir`; returns its path.
#[cfg_attr(not(test), allow(dead_code))]
pub fn export_to(dir: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    export_from_dirs(&[dir.to_path_buf()], dest_dir)
}

/// Zips the logs (both user GUI logs and machine-wide RemoteViewSvc logs) to Desktop.
pub fn export_logs() -> Result<PathBuf, String> {
    let dest = target_dir().ok_or("Không tìm thấy thư mục Desktop")?;
    let mut dirs = vec![logfile::logs_dir()];
    if let Some(shared) = logfile::shared_logs_dir() {
        if shared != dirs[0] {
            dirs.push(shared);
        }
    }
    export_from_dirs(&dirs, &dest)
}

/// Opens Explorer with `path` selected (the containing folder elsewhere).
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(not(windows))]
    {
        let dir = path.parent().unwrap_or(path);
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(dir).spawn();
        #[cfg(not(target_os = "macos"))]
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn zip_layout_is_valid() {
        let t = logfile::utc_from_unix(1_700_000_000, 0);
        let z = zip_bytes(&[("a.log".into(), b"hello".to_vec()), ("b.log".into(), vec![])], &t);
        assert_eq!(&z[..4], b"PK\x03\x04");
        let eocd = z.len() - 22;
        assert_eq!(&z[eocd..eocd + 4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([z[eocd + 10], z[eocd + 11]]), 2);
        let cd_off = u32::from_le_bytes(z[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        assert_eq!(&z[cd_off..cd_off + 4], b"PK\x01\x02");
    }

    #[test]
    fn exports_current_previous_and_android_logs() {
        let base = std::env::temp_dir().join(format!("rv-export-test-{}", std::process::id()));
        let (logs, out) = (base.join("logs"), base.join("out"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&logs).unwrap();
        for n in ["remoteview-20260101-000001.log", "remoteview-20260101-000002.log", "remoteview-20260101-000003.log", "android-tab.log"] {
            std::fs::write(logs.join(n), n.as_bytes()).unwrap();
        }
        let zip = export_to(&logs, &out).unwrap();
        assert!(zip.file_name().unwrap().to_string_lossy().starts_with("RemoteView-logs-"));
        let bytes = std::fs::read(&zip).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("remoteview-20260101-000003.log") && text.contains("remoteview-20260101-000002.log"));
        assert!(!text.contains("remoteview-20260101-000001.log")); // only current + previous
        assert!(text.contains("android-tab.log") && text.contains("info.txt"));
        assert!(export_to(&base.join("none"), &out).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
