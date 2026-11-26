//! Embeds the git commit (or CI's `GITHUB_SHA`) as `RV_GIT_SHA` for the logs.

fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=RV_GIT_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let sha = std::env::var("RV_GIT_SHA")
        .ok()
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(12).collect::<String>())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RV_GIT_SHA={sha}");
}
