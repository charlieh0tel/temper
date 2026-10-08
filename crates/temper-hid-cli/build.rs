//! Stamps the build with the commit it came from, as `TEMPER_VERSION`.
//!
//! One string for `--version`, the daemon's startup line and the Debian
//! package's version, so all of them name the same build.  Ordered for
//! apt with no version bump after a release (`~` sorts before
//! everything, `+` after the release it follows):
//!
//!     0.1.0~git12.gabc1234  <  0.1.0  <  0.1.0+git5.gdef5678  <  0.1.1
//!
//! A clean build on its release tag is that release.  Without git (a
//! source tarball) it is the crate version alone.

use std::env;
use std::process::Command;

/// Runs git, returning its trimmed output if it succeeded.
fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn main() {
    // Rebuild when the commit or the tags change.
    // The index changes when files are staged; edits not yet staged are
    // caught by `make deb`, which touches this file to force a rerun.
    for path in [
        ".git/HEAD",
        ".git/refs/heads",
        ".git/refs/tags",
        ".git/packed-refs",
        ".git/index",
    ] {
        println!("cargo::rerun-if-changed=../../{path}");
    }
    let package = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    // Untracked files are not part of the build.
    let dirty = if git(&["status", "--porcelain", "--untracked-files=no"]).is_some() {
        ".dirty"
    } else {
        ""
    };
    // "v0.1.0-5-gabc1234": the nearest release tag, commits since, commit.
    let described = git(&["describe", "--tags", "--long", "--match", "v[0-9]*"]);
    let parsed = described.as_deref().and_then(|text| {
        let mut parts = text.rsplitn(3, '-');
        let commit = parts.next()?;
        let count = parts.next()?;
        let tag = parts.next()?.strip_prefix('v')?;
        Some((tag.to_owned(), count.to_owned(), commit.to_owned()))
    });
    let version = match parsed {
        Some((tag, count, _)) if count == "0" && dirty.is_empty() && tag == package => package,
        Some((tag, count, commit)) => format!("{tag}+git{count}.{commit}{dirty}"),
        None => match (
            git(&["rev-list", "--count", "HEAD"]),
            git(&["rev-parse", "--short", "HEAD"]),
        ) {
            (Some(count), Some(commit)) => format!("{package}~git{count}.g{commit}{dirty}"),
            _ => package,
        },
    };
    println!("cargo::rustc-env=TEMPER_VERSION={version}");
}
