#!/usr/bin/env bash
# Build .deb, .rpm and Arch packages from a cargo-dist Linux tarball.
set -euo pipefail

[ $# -eq 3 ] || { echo "usage: $0 <rune-cli-<triple>.tar.xz> <version> <outdir>" >&2; exit 2; }
tarball=$(realpath "$1")
version=${2#v}
outdir=$3

[ -f "$tarball" ] || { echo "package: tarball not found: $tarball" >&2; exit 1; }
[ -n "$version" ] || { echo "package: empty version" >&2; exit 1; }

base=$(basename "$tarball")
triple=${base#rune-cli-}
triple=${triple%.tar.xz}
case "$triple" in
  x86_64-unknown-linux-gnu)  ARCH=amd64 ;;
  aarch64-unknown-linux-gnu) ARCH=arm64 ;;
  *) echo "package: unsupported tarball name '$base' (expected rune-cli-<x86_64|aarch64>-unknown-linux-gnu.tar.xz)" >&2; exit 1 ;;
esac

yaml=$(realpath "$(dirname "${BASH_SOURCE[0]}")/nfpm.yaml")
mkdir -p "$outdir"
outdir=$(realpath "$outdir")

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
tar -xJf "$tarball" -C "$stage" --strip-components=1

for f in rune LICENSE.txt README.md; do
  [ -f "$stage/$f" ] || { echo "package: $f missing from $base" >&2; exit 1; }
done

export ARCH VERSION=$version
cd "$stage"
for packager in deb rpm archlinux; do
  nfpm package -p "$packager" -f "$yaml" -t "$outdir"
done
