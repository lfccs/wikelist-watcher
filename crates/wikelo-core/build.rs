//! Records the git commit in the binary (`wikelo_core::GIT_COMMIT`), so a dataset says
//! which extractor build produced it. Order: `git rev-parse` of the checkout, then
//! `GITHUB_SHA` (CI / Docker build arg), then `dev`.

use std::path::Path;
use std::process::Command;

fn main() {
    let from_git = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    let commit = from_git
        .or_else(|| std::env::var("GITHUB_SHA").ok().map(|s| s.chars().take(9).collect::<String>()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "dev".into());
    println!("cargo:rustc-env=WIKELO_GIT_COMMIT={commit}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    // `../../.git` when this crate is built from the public repository, `../../../.git` when it
    // is the `watcher/` subtree of the server repository.
    for p in ["../../.git/HEAD", "../../.git/refs/heads", "../../../.git/HEAD", "../../../.git/refs/heads"] {
        if Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }
}
