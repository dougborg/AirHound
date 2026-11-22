// SPDX-License-Identifier: GPL-2.0-only
//! Capture source provenance, not an inferred release from the package version.
use std::{env, path::PathBuf, process::Command};
fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    // Do not accidentally identify a source archive as its enclosing checkout.
    if git(&["ls-files", "--error-unmatch", "Cargo.toml"]).is_none() {
        println!("cargo:rustc-env=MT7610U_GIT_REVISION=");
        println!("cargo:rustc-env=MT7610U_SOURCE_MODIFIED=unknown");
        println!("cargo:rustc-env=MT7610U_SOURCE_TAG=");
        return;
    }
    // Worktrees have a .git file; ask git for the actual HEAD/index paths.
    for name in ["HEAD", "index", "packed-refs", "refs/tags", "refs/heads"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={}", manifest.join(path).display());
        }
    }
    if let Some(root) = git(&["rev-parse", "--show-toplevel"]) {
        if let Some(files) = git(&["-C", &root, "ls-files"]) {
            for file in files.lines().filter(|file| !file.contains(['\n', '\r'])) {
                println!(
                    "cargo:rerun-if-changed={}",
                    PathBuf::from(&root).join(file).display()
                );
            }
        }
    }
    let revision = git(&["rev-parse", "HEAD"])
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    let modified = git(&["status", "--porcelain", "--untracked-files=no"]).map(|s| !s.is_empty());
    let tag = git(&["describe", "--tags", "--exact-match", "HEAD"])
        .filter(|s| s.len() <= 128 && !s.chars().any(char::is_control));
    println!(
        "cargo:rustc-env=MT7610U_GIT_REVISION={}",
        revision.as_deref().unwrap_or("")
    );
    println!(
        "cargo:rustc-env=MT7610U_SOURCE_MODIFIED={}",
        modified
            .map(|b| if b { "true" } else { "false" })
            .unwrap_or("unknown")
    );
    println!(
        "cargo:rustc-env=MT7610U_SOURCE_TAG={}",
        tag.as_deref().unwrap_or("")
    );
}
