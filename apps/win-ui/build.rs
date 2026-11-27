//! Build script for RemoteView Windows UI (`rv-win-ui`).
//!
//! When targeting Windows (`CARGO_CFG_TARGET_OS == "windows"`):
//! - Generates a multi-resolution `.ico` (16x16, 32x32, 48x48) in `OUT_DIR`
//!   matching RemoteView's programmatic monitor icon (`util::icon_rgba`).
//! - Copies `assets/app.manifest` into `OUT_DIR` and emits `remoteview.rc`
//!   embedding icon resource `1` and manifest resource `1 24`.
//! - Compiles the `.rc` with `windres` (GNU/MinGW) or `rc.exe` / `llvm-rc`
//!   (MSVC) using zero external build-dependencies, then links the resulting
//!   resource into the binary via `cargo:rustc-link-arg-bins=...`.
//!
//! On non-Windows targets this script is a fast no-op.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = match env::var_os("OUT_DIR") {
        Some(d) => PathBuf::from(d),
        None => return,
    };
    let manifest_dir = match env::var_os("CARGO_MANIFEST_DIR") {
        Some(d) => PathBuf::from(d),
        None => return,
    };

    let ico_path = out_dir.join("remoteview.ico");
    let ico_bytes = build_ico(&[16, 32, 48]);
    if let Err(e) = fs::write(&ico_path, &ico_bytes) {
        println!("cargo:warning=failed to write {}: {e}", ico_path.display());
        return;
    }

    let manifest_src = manifest_dir.join("assets").join("app.manifest");
    let manifest_dst = out_dir.join("app.manifest");
    if let Err(e) = fs::copy(&manifest_src, &manifest_dst) {
        println!(
            "cargo:warning=failed to copy {} to {}: {e}",
            manifest_src.display(),
            manifest_dst.display()
        );
        return;
    }

    let rc_path = out_dir.join("remoteview.rc");
    let rc_content = "1 ICON \"remoteview.ico\"\n1 24 \"app.manifest\"\n";
    if let Err(e) = fs::write(&rc_path, rc_content) {
        println!("cargo:warning=failed to write {}: {e}", rc_path.display());
        return;
    }

    compile_rc(&out_dir);
}

/// Compiles `remoteview.rc` inside `out_dir` and emits `cargo:rustc-link-arg-bins`
/// for the resulting `.res` (MSVC) or `.res.o` (GNU/MinGW) resource object.
/// Warns gracefully instead of failing if no resource compiler is available.
fn compile_rc(out_dir: &Path) {
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if target_env == "msvc" {
        let res_name = "remoteview.res";
        let res_path = out_dir.join(res_name);

        if let Some(rc_exe) = find_rc_exe(&target_arch) {
            let status = Command::new(&rc_exe)
                .current_dir(out_dir)
                .args(["/nologo", "/fo", res_name, "remoteview.rc"])
                .status();
            match status {
                Ok(s) if s.success() => {
                    println!("cargo:rustc-link-arg-bins={}", res_path.display());
                    return;
                }
                Ok(s) => {
                    println!(
                        "cargo:warning=rc compiler {} exited with {s}; PE icon/manifest will not be embedded",
                        rc_exe.display()
                    );
                    return;
                }
                Err(e) => {
                    println!(
                        "cargo:warning=failed to invoke {}: {e}; PE icon/manifest will not be embedded",
                        rc_exe.display()
                    );
                    return;
                }
            }
        }

        // Fallback: llvm-rc (useful for cross-compiling MSVC targets with cargo-xwin)
        if let Ok(s) = Command::new("llvm-rc")
            .current_dir(out_dir)
            .args(["/fo", res_name, "remoteview.rc"])
            .status()
        {
            if s.success() {
                println!("cargo:rustc-link-arg-bins={}", res_path.display());
                return;
            }
            println!(
                "cargo:warning=llvm-rc exited with {s}; PE icon/manifest will not be embedded"
            );
            return;
        }

        println!("cargo:warning=neither rc.exe nor llvm-rc found; PE icon/manifest will not be embedded");
        return;
    }

    // GNU / MinGW / gnullvm
    let obj_name = "remoteview.res.o";
    let obj_path = out_dir.join(obj_name);
    let candidates: &[&str] = match target_arch.as_str() {
        "x86_64" => &["x86_64-w64-mingw32-windres", "windres", "llvm-windres"],
        "x86" => &["i686-w64-mingw32-windres", "windres", "llvm-windres"],
        "aarch64" => &["aarch64-w64-mingw32-windres", "llvm-windres", "windres"],
        _ => &["windres", "llvm-windres"],
    };

    for tool in candidates {
        match Command::new(tool)
            .current_dir(out_dir)
            .args(["-i", "remoteview.rc", "-o", obj_name, "-O", "coff"])
            .status()
        {
            Ok(s) if s.success() => {
                println!("cargo:rustc-link-arg-bins={}", obj_path.display());
                return;
            }
            Ok(s) => {
                println!(
                    "cargo:warning={tool} exited with {s}; PE icon/manifest will not be embedded"
                );
                return;
            }
            Err(_) => continue,
        }
    }

    println!(
        "cargo:warning=no windres tool found for {target_arch}-{target_env}; PE icon/manifest will not be embedded"
    );
}

/// Locates `rc.exe` on `PATH` or under the newest Windows 10/11 SDK directory.
fn find_rc_exe(target_arch: &str) -> Option<PathBuf> {
    if let Ok(s) = Command::new("rc.exe").arg("/?").output() {
        // `rc.exe /?` succeeds or exits non-zero, but spawning it succeeded so it is on PATH.
        let _ = s;
        return Some(PathBuf::from("rc.exe"));
    }

    let sdk_arch = match target_arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "x64",
    };

    let mut roots = Vec::new();
    if let Some(pf86) = env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(pf86).join("Windows Kits").join("10").join("bin"));
    }
    if let Some(pf) = env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(pf).join("Windows Kits").join("10").join("bin"));
    }
    roots.push(PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin"));

    for bin_root in roots {
        let Ok(entries) = fs::read_dir(&bin_root) else {
            continue;
        };
        let mut versions: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        // Sort descending so the newest SDK version (e.g. 10.0.26100.0) is checked first.
        versions.sort();
        versions.reverse();

        for ver_dir in versions {
            let candidate = ver_dir.join(sdk_arch).join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        // Legacy layout: Windows Kits\10\bin\<arch>\rc.exe
        let fallback = bin_root.join(sdk_arch).join("rc.exe");
        if fallback.is_file() {
            return Some(fallback);
        }
    }

    None
}

/// Builds a standard multi-image `.ico` containing 32-bit BGRA BMP-in-ICO DIBs.
fn build_ico(sizes: &[u32]) -> Vec<u8> {
    let mut images: Vec<(u32, Vec<u8>)> = Vec::with_capacity(sizes.len());
    for &size in sizes {
        images.push((size, build_ico_dib(size)));
    }

    let header_len = 6 + 16 * images.len();
    let total_len = header_len + images.iter().map(|(_, d)| d.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total_len);

    // ICONDIR (6 bytes)
    out.extend_from_slice(&0u16.to_le_bytes()); // idReserved
    out.extend_from_slice(&1u16.to_le_bytes()); // idType = 1 (ICO)
    out.extend_from_slice(&(images.len() as u16).to_le_bytes()); // idCount

    // ICONDIRENTRY (16 bytes each)
    let mut offset = header_len as u32;
    for (size, dib) in &images {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dim); // bWidth
        out.push(dim); // bHeight
        out.push(0); // bColorCount
        out.push(0); // bReserved
        out.extend_from_slice(&1u16.to_le_bytes()); // wPlanes
        out.extend_from_slice(&32u16.to_le_bytes()); // wBitCount
        out.extend_from_slice(&(dib.len() as u32).to_le_bytes()); // dwBytesInRes
        out.extend_from_slice(&offset.to_le_bytes()); // dwImageOffset
        offset += dib.len() as u32;
    }

    // Image payloads
    for (_, dib) in images {
        out.extend_from_slice(&dib);
    }

    out
}

/// Builds a single 32-bit BGRA BMP-in-ICO payload:
/// BITMAPINFOHEADER (40 bytes, biHeight = 2 * size) + bottom-up BGRA XOR mask + bottom-up 1bpp AND mask.
fn build_ico_dib(size: u32) -> Vec<u8> {
    let rgba = icon_rgba(size);
    let xor_bytes = (size * size * 4) as usize;
    let and_row_stride = (size.div_ceil(32) * 4) as usize;
    let and_bytes = and_row_stride * size as usize;
    let image_size = (xor_bytes + and_bytes) as u32;

    let mut dib = Vec::with_capacity(40 + xor_bytes + and_bytes);

    // BITMAPINFOHEADER (40 bytes)
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&((size * 2) as i32).to_le_bytes()); // biHeight (XOR + AND)
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression (BI_RGB)
    dib.extend_from_slice(&image_size.to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    // XOR mask: bottom-up BGRA pixels
    for y in (0..size).rev() {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let r = rgba[idx];
            let g = rgba[idx + 1];
            let b = rgba[idx + 2];
            let a = rgba[idx + 3];
            dib.extend_from_slice(&[b, g, r, a]);
        }
    }

    // AND mask: bottom-up 1bpp rows padded to 4-byte boundaries (1 = transparent, 0 = opaque)
    let mut row = vec![0u8; and_row_stride];
    for y in (0..size).rev() {
        row.fill(0);
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            if rgba[idx + 3] < 128 {
                let byte_idx = (x / 8) as usize;
                let bit = 7 - (x % 8);
                row[byte_idx] |= 1u8 << bit;
            }
        }
        dib.extend_from_slice(&row);
    }

    dib
}

/// Programmatic app icon matching `src/util.rs::icon_rgba`:
/// blue rounded square with a white monitor screen, stand, and play wedge.
fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let inside_round = |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    for yi in 0..size {
        for xi in 0..size {
            let (x, y) = (xi as f32 + 0.5, yi as f32 + 0.5);
            let i = ((yi * size + xi) * 4) as usize;
            if !inside_round(x, y, 0.0, 0.0, s, s, s * 0.22) {
                continue;
            }
            let t = y / s;
            let mut c = [
                (40.0 + 30.0 * t) as u8,
                (110.0 - 20.0 * t) as u8,
                (240.0 - 30.0 * t) as u8,
                255,
            ];
            // screen outline
            let outer = inside_round(x, y, s * 0.16, s * 0.24, s * 0.84, s * 0.66, s * 0.06);
            let inner = inside_round(x, y, s * 0.22, s * 0.30, s * 0.78, s * 0.60, s * 0.03);
            if outer && !inner {
                c = [255, 255, 255, 255];
            }
            // stand
            if (s * 0.40..s * 0.60).contains(&x) && (s * 0.66..s * 0.74).contains(&y)
                || (s * 0.30..s * 0.70).contains(&x) && (s * 0.74..s * 0.80).contains(&y)
            {
                c = [255, 255, 255, 255];
            }
            // play wedge
            let (px0, py0) = (s * 0.43, s * 0.36);
            let py1 = s * 0.54;
            let tipx = s * 0.60;
            if x >= px0 && x <= tipx {
                let f = (x - px0) / (tipx - px0);
                let mid = (py0 + py1) / 2.0;
                let half = (py1 - py0) / 2.0 * (1.0 - f);
                if (y - mid).abs() <= half {
                    c = [255, 255, 255, 255];
                }
            }
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    px
}
