#!/usr/bin/env bash
# Packages one target's release build for distribution: the binary, the
# README, the changelog and both licences in one archive, with a SHA-256
# checksum beside it in `sha256sum -c` format. Runs under bash on all three
# platforms. Windows gets a zip because that is what Windows opens; the
# others get a tar.gz.
#
# usage: .github/scripts/package.sh <target-triple> <output-dir>
set -euo pipefail

target=${1:?usage: package.sh <target-triple> <output-dir>}
out=${2:?usage: package.sh <target-triple> <output-dir>}

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
name="knowmoretabs-$version-$target"
bin=knowmoretabs
case "$target" in *windows*) bin=knowmoretabs.exe ;; esac

work=$(mktemp -d)
mkdir -p "$work/$name" "$out"
cp "target/$target/release/$bin" README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE "$work/$name/"

case "$target" in
  *windows*)
    archive="$name.zip"
    (cd "$work" && 7z a -bso0 -tzip "$archive" "$name")
    ;;
  *)
    archive="$name.tar.gz"
    tar -C "$work" -czf "$work/$archive" "$name"
    ;;
esac
mv "$work/$archive" "$out/"
rm -rf "$work"

cd "$out"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$archive" > "$archive.sha256"
else
  shasum -a 256 "$archive" > "$archive.sha256"
fi
cat "$archive.sha256"
