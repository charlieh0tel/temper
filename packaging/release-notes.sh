#!/bin/sh
# The newest entry of packaging/debian/changelog as Markdown, for a
# GitHub release's notes: its bullets as a list, then a link comparing
# its tag with the previous entry's.  Run from the top of the tree.
set -eu

changelog=packaging/debian/changelog
repository=$(sed -n 's/^repository = "\(.*\)"$/\1/p' Cargo.toml)

# The upstream version of each entry, newest first: "0.1.0" from
# "tempered (0.1.0-1) unstable; urgency=low".
versions=$(sed -n 's/^[a-z0-9.+-]* (\([^-)]*\)-[^)]*).*/\1/p' "$changelog")
this=$(echo "$versions" | sed -n 1p)
previous=$(echo "$versions" | sed -n 2p)

# The first entry's bullets, each joined onto one line.
awk '
  NR == 1 { next }
  /^ -- / { exit }
  /^  \* / { if (item != "") print item; item = "- " substr($0, 5); next }
  /^    / { item = item " " substr($0, 5) }
  END { if (item != "") print item }
' "$changelog"

if [ -n "$previous" ]; then
    printf '\n**Full Changelog**: %s/compare/v%s...v%s\n' "$repository" "$previous" "$this"
fi
