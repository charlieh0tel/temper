# Releasing

A release is a pushed `v*` tag.  The `release` workflow then audits
`Cargo.lock`, builds the `tempered` .deb for amd64 and arm64, creates
the GitHub release with the changelog entry as its notes, and triggers
a rebuild of `charlieh0tel/apt-repo`.  Nothing is published to
crates.io.

1. On a clean, up-to-date `main`, check advisories (`cargo audit`);
   commit any lockfile update on its own.
2. Set `version` in the workspace `Cargo.toml` to the release, or pass
   `VERSION=x.y.z`.
3. `make release`: writes the changelog entry (opens `$EDITOR`),
   commits, and tags `vx.y.z`.  `make release-notes` previews the
   notes.
4. `make deb` builds the package locally to check it.
5. `git push origin main && git push origin vx.y.z`.

## Versions between releases

`build.rs` stamps every build from `git describe`, so `tempered
--version`, the daemon's startup line and `make deb` agree.  A clean
build on its release tag is the release; anything else is a snapshot:
`0.1.0~git12.gabc1234` before the first tag, `0.1.0+git5.gabc1234`
after `v0.1.0`, with `.dirty` for uncommitted changes.  apt orders them
around the releases without a version bump after each one.

