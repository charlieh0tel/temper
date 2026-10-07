# Releasing

A release is a pushed `v*` tag.  The `release` workflow then checks
that the tag, `Cargo.toml` and the changelog name one version, audits
`Cargo.lock`, builds the `tempered` .deb for amd64 and arm64, creates
the GitHub release with the changelog entry as its notes, publishes
the `tempered-hid` library to crates.io (Trusted Publishing; no token
stored here), and triggers a rebuild of `charlieh0tel/apt-repo`.  The
library shares the workspace version, so every release publishes it,
as in usbrelay-rs and ut325f-rs.

1. On a clean, up-to-date `main`, check advisories (`cargo audit`);
   commit any lockfile update on its own.
2. `make release VERSION=x.y.z`: sets the workspace `Cargo.toml`
   version, writes the changelog entry (opens `$EDITOR`), commits, and
   tags `vx.y.z`.  It refuses a dirty tree, so do not edit
   `Cargo.toml` by hand first.  `make release-notes` previews the
   notes.
3. `make deb` builds the package locally to check it.
4. `git push origin main && git push origin vx.y.z`.

Never move or re-push a published tag, and remember crates.io versions
are permanent: a version can be yanked but not replaced.  If a release
fails after the tag is pushed, fix it and release the next patch.

## The first release

crates.io's Trusted Publishing cannot create a crate, so 1.0.0 was
published by hand (`cargo publish -p tempered-hid`) and the trusted
publisher (repository `charlieh0tel/tempered-hid`, workflow
`release.yml`) set up afterward; that tag's `publish-crate` job failed
on the version already being there.  Later tags publish from CI.

## Versions between releases

`build.rs` stamps every build from `git describe`, so `tempered
--version`, the daemon's startup line and `make deb` agree.  A clean
build on its release tag is the release; anything else is a snapshot:
`1.0.0+git5.gabc1234` five commits after `v1.0.0`, or
`x.y.z~gitN.g...` before any tag, with `.dirty` for uncommitted
changes (`make deb` reruns `build.rs` so the stamp sees them).  apt orders them
around the releases without a version bump after each one.

