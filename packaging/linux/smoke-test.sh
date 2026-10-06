#!/usr/bin/env bash
# Install each package with its native package manager in a clean container
# and check that `rune --version` reports the expected version.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <pkgdir> <version>" >&2; exit 2; }
pkgdir=$(realpath "$1")
version=${2#v}

if command -v docker >/dev/null; then engine=docker
elif command -v podman >/dev/null; then engine=podman
else echo "smoke-test: neither docker nor podman found" >&2; exit 1
fi

case "$(uname -m)" in
  x86_64)  debarch=amd64 rpmarch=x86_64 ;;
  aarch64) debarch=arm64 rpmarch=aarch64 ;;
  *) echo "smoke-test: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

# run_in <image> <install commands>; asserts the version string afterwards.
run_in() {
  local image=$1 install=$2 out
  echo "== $image"
  out=$("$engine" run --rm -v "$pkgdir:/pkgs:ro" -w /pkgs "$image" \
    sh -euc "{ $install; } >&2; rune --version")
  if [ "$out" != "rune $version" ]; then
    echo "smoke-test: $image printed '$out', want 'rune $version'" >&2
    exit 1
  fi
  echo "ok: $out"
}

run_in docker.io/library/ubuntu:22.04 \
  "apt-get update && apt-get install -y ./rune_*_${debarch}.deb"
run_in docker.io/library/fedora:latest \
  "dnf install -y ./rune-*.${rpmarch}.rpm"

if [ "$(uname -m)" = x86_64 ]; then
  run_in docker.io/library/archlinux:latest \
    "pacman -Syu --noconfirm && pacman -U --noconfirm ./rune-*-x86_64.pkg.tar.zst"
else
  echo "skipped arch: no official arm64 image"
fi
