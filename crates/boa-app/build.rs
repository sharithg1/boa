//! Build metadata for About Boa / "Copy version info": the commit the app is
//! built from and its date (the Tauri app's `BOA_BUILD_COMMIT` /
//! `BOA_BUILD_DATE`). Empty when git is unavailable (e.g. a source tarball);
//! the environment variables of the same names override (release scripts).

use std::process::Command;

fn git(args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn main() {
    println!("cargo:rerun-if-env-changed=BOA_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=BOA_BUILD_DATE");
    let commit = std::env::var("BOA_BUILD_COMMIT")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| git(&["rev-parse", "--short=12", "HEAD"]));
    let date = std::env::var("BOA_BUILD_DATE")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| git(&["show", "-s", "--format=%cI", "HEAD"]));
    println!("cargo:rustc-env=BOA_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=BOA_BUILD_DATE={date}");
    // Rebuild when HEAD moves (best effort: the worktree's git dir may differ).
    let head = git(&["rev-parse", "--git-path", "HEAD"]);
    if !head.is_empty() {
        println!("cargo:rerun-if-changed={head}");
    }
}
