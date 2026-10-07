# CI calls the shared rust-ci.yml; "make ci" runs the same checks locally.

CARGO ?= cargo

.PHONY: all build ci fmt fmt-check clippy test test-hw doc clean deb release release-notes

all: build

build:
	$(CARGO) build --workspace --all-targets

# What CI runs.  Keep this the whole of it.
ci: fmt-check clippy test

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything that runs without root or a stick.
test:
	$(CARGO) test --workspace

# The root-only tests: the real kernel through /dev/uhid, a fake stick,
# and the attached stick.  The test binaries run under sudo; the
# daemon tests start target/debug/tempered, so build first.  Never run
# in CI.  They create HID temperature sensors, so stop tempered@ first:
# the kernel cannot handle two (docs/daemon.md).
test-hw: build
	$(CARGO) test --workspace --config "target.'cfg(unix)'.runner = 'sudo'" -- --ignored

doc:
	$(CARGO) doc --workspace --no-deps

clean:
	$(CARGO) clean

# Requires cargo-deb: cargo install cargo-deb
deb:
	$(CARGO) build --release --workspace
	$(CARGO) deb -p tempered-bin --no-build -q

# Cut a release: one version, in Cargo.toml and the changelog, tagged.
# Refuses a dirty tree, since the tag would name a commit that does not
# contain what was built.  See RELEASING.md.
VERSION ?= $(shell sed -n '0,/^version = "\(.*\)"/s//\1/p' Cargo.toml)

release:
	@echo "$(VERSION)" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$$' || \
	    { echo "VERSION must be x.y.z, not $(VERSION)" >&2; exit 2; }
	@test -z "$$(git status --porcelain)" || { echo "the tree is dirty" >&2; exit 2; }
	@git rev-parse -q --verify "refs/tags/v$(VERSION)" >/dev/null && \
	    { echo "v$(VERSION) already exists" >&2; exit 2; } || true
	sed -i '0,/^version = ".*"/s//version = "$(VERSION)"/' Cargo.toml
	@head -1 packaging/debian/changelog | grep -q "($(VERSION)-1)" || { \
	    printf '%s\n\n  * \n\n -- %s  %s\n\n%s\n' \
	        'tempered ($(VERSION)-1) unstable; urgency=low' \
	        'Christopher Hoover <ch@murgatroid.com>' \
	        "$$(date -R)" \
	        "$$(cat packaging/debian/changelog)" > packaging/debian/changelog.new && \
	    mv packaging/debian/changelog.new packaging/debian/changelog && \
	    $${EDITOR:-vi} packaging/debian/changelog; }
	$(CARGO) build --workspace
	git add Cargo.toml Cargo.lock packaging/debian/changelog
	git commit -m "Release $(VERSION)"
	git tag -a "v$(VERSION)" -m "Release $(VERSION)"
	@echo "Tagged v$(VERSION).  Push to build and publish:"
	@echo "    git push origin main && git push origin v$(VERSION)"

# The GitHub release's notes, from the newest changelog entry.
release-notes:
	@./packaging/release-notes.sh
