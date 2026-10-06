#!/usr/bin/env bash
# Install the pinned nfpm release into <bindir>, verifying a hardcoded sha256.
set -euo pipefail

NFPM_VERSION=2.47.0
SHA_X86_64=0660ca602b2d2d2ae4781a06c692b3eeb9d437ffea05b831d76e41f4a3188783
SHA_ARM64=1c0f5f2999b9a974bfb04fdb0cc3306096de530ac5dbb25d739cc5f5219c919c

[ $# -eq 1 ] || { echo "usage: $0 <bindir>" >&2; exit 2; }
bindir=$1

case "$(uname -m)" in
  x86_64)  asset=Linux_x86_64; want=$SHA_X86_64 ;;
  aarch64) asset=Linux_arm64;  want=$SHA_ARM64 ;;
  *) echo "install-nfpm: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

file=nfpm_${NFPM_VERSION}_${asset}.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$file" \
  "https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VERSION}/${file}"

got=$(sha256sum "$tmp/$file" | cut -d' ' -f1)
if [ "$got" != "$want" ]; then
  echo "install-nfpm: sha256 mismatch for $file: got $got, want $want" >&2
  exit 1
fi

mkdir -p "$bindir"
tar -xzf "$tmp/$file" -C "$tmp" nfpm
install -m 0755 "$tmp/nfpm" "$bindir/nfpm"
echo "installed nfpm ${NFPM_VERSION} to $bindir/nfpm"
