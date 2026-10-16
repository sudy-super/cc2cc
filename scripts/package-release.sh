#!/bin/sh
set -eu
[ "$#" -eq 1 ] || { printf 'Usage: %s TARGET\n' "$0" >&2; exit 2; }
target=$1
case "$target" in aarch64-apple-darwin|x86_64-unknown-linux-musl) ;; *) exit 2 ;; esac
binary=target/$target/release/cc2cc
[ -f "$binary" ]
mkdir -p dist
stage=$(mktemp -d "${TMPDIR:-/tmp}/cc2cc-package.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM
mkdir -p "$stage/skills/cc2cc"
cp "$binary" "$stage/cc2cc"
chmod 755 "$stage/cc2cc"
cp LICENSE THIRD_PARTY_NOTICES.md "$stage/"
cp skills/cc2cc/SKILL.md "$stage/skills/cc2cc/"
# macOS archive metadata is not useful to Linux users or the installer.
COPYFILE_DISABLE=1 tar -czf "dist/cc2cc-$target.tar.gz" -C "$stage" cc2cc LICENSE THIRD_PARTY_NOTICES.md skills/cc2cc/SKILL.md
