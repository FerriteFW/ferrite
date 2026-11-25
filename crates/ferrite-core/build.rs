//! The build ID (SELF-5): the git commit the firmware was built from, short, with "+" when the tree had
//! changes not committed. Read from the local repository only; "unknown" outside one.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    // FERRITE_COMMIT: set by tools/remote.sh, whose copy has no .git of its own (the commit already carries "+" if dirty)
    println!("cargo:rerun-if-env-changed=FERRITE_COMMIT");
    if let Ok(c) = std::env::var("FERRITE_COMMIT") {
        println!("cargo:rustc-env=FERRITE_BUILD={c}");
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }
    let sha = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    // --no-optional-locks: git status otherwise refreshes the index, which (watched below) rebuilt everything
    // after ferrite-core on every build
    let dirty = git(&["--no-optional-locks", "status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    println!("cargo:rustc-env=FERRITE_BUILD={sha}{}", if dirty { "+" } else { "" });
    // Rebuild when HEAD moves or the index changes (a commit, a checkout, a staged edit).
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        for f in ["HEAD", "index", "packed-refs"] {
            println!("cargo:rerun-if-changed={dir}/{f}");
        }
        if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={dir}/{r}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
