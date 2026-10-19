#!/usr/bin/env bash
# Idempotent CI toolchain bootstrap shared by the Forgejo and GitHub workflows:
# rustup + the pinned toolchain with clippy and rustfmt, then cargo-nextest.
set -euo pipefail

if ! command -v rustup >/dev/null; then
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs |
        sh -s -- -y --profile minimal --default-toolchain none
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    if [ -n "${GITHUB_PATH:-}" ]; then
        echo "${CARGO_HOME:-$HOME/.cargo}/bin" >>"$GITHUB_PATH"
    fi
fi

rustup toolchain install --profile minimal --component clippy --component rustfmt
rustc --version
cargo --version

if ! command -v cargo-nextest >/dev/null; then
    case "$(uname -s)" in
    Linux) nextest_platform=linux ;;
    Darwin) nextest_platform=mac ;;
    *)
        echo "unsupported OS for cargo-nextest: $(uname -s)" >&2
        exit 1
        ;;
    esac
    bin="${CARGO_HOME:-$HOME/.cargo}/bin"
    mkdir -p "$bin"
    curl -fsSL "https://get.nexte.st/latest/$nextest_platform" | tar -xzf - -C "$bin"
fi
cargo nextest --version
