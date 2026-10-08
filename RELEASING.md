# Releasing

A release is a pushed `v*` tag.  The `release` workflow then checks
that the tag, `Cargo.toml` (and its `temper-hid` dependency) and the
changelog name one version, audits `Cargo.lock`, builds the `temper`
and `temper-iio` .debs for amd64 and arm64, creates the GitHub release
with the changelog entry as its notes, publishes the `temper-hid`
library and the `temper-hid-cli` tool to crates.io (Trusted
Publishing; no token stored here), and triggers a rebuild of
`charlieh0tel/apt-repo`.  Both crates share the workspace version, so
every release publishes them, as in usbrelay-rs and ut325f-rs.  One
Debian changelog, headed by the source name `temper`, serves both
packages.

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

## A new crate's first release

crates.io's Trusted Publishing cannot create a crate, so a new
crate's first version is published by hand from the release commit
(`cargo publish -p <crate>`, `temper-hid` before `temper-hid-cli`),
before the tag is pushed, and its trusted publisher (repository
`charlieh0tel/temper`, workflow `release.yml`) set up afterward.  The
`publish-crate` job skips a version crates.io already has.  The
library's first release, as `tempered-hid` 1.0.0, went this way.

## Versions between releases

`build.rs` (in `temper-hid-cli`, shared by `temper-iio`) stamps every
build from `git describe`, so `temper --version`, `temper-iio
--version`, the daemon's startup line and `make deb` agree.  A clean
build on its release tag is the release; anything else is a snapshot:
`1.0.0+git5.gabc1234` five commits after `v1.0.0`, or
`x.y.z~gitN.g...` before any tag, with `.dirty` for uncommitted
changes (`make deb` reruns `build.rs` so the stamp sees them).  apt orders them
around the releases without a version bump after each one.

