# Releasing

Rune's release pipeline builds an `aarch64-apple-darwin` binary via `cargo-dist` on a
git tag push, publishes it as a GitHub Release, and pushes the `rune` Homebrew formula
to `aka-rider/homebrew-tap`.

## One-time setup

Create a GitHub personal access token with `repo` scope that can push to
`aka-rider/homebrew-tap`, then add it as repo secret `HOMEBREW_TAP_TOKEN` on
`aka-rider/rune`. This is a different secret name from the Go pipeline's
`HOMEBREW_TAP_GITHUB_TOKEN`; the same PAT can back both secrets.

## Per release

1. Bump `version` in `crates/rune-cli/Cargo.toml` to the new release version (e.g.
   `1.2.0`), then run `cargo build -p rune-cli` to refresh `Cargo.lock`.
2. Commit the bump, merge to `main`, and push. The eventual tag must match the crate
   version (`v1.2.0` ↔ `1.2.0`) and must point at a commit on `main` that contains the
   Rust tree — tagging before that work reaches pushed `main` would release the Go
   tree instead.
3. Tag and push:
   ```sh
   git tag v1.2.0
   git push origin v1.2.0
   ```
   CI builds the release, creates the GitHub Release, and pushes the `rune` formula to
   `aka-rider/homebrew-tap`.
4. Verify:
   ```sh
   brew update
   brew install aka-rider/tap/rune
   rune --version
   ```
