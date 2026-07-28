# Map EOPNOTSUPP from renamex_np to ErrUnsupported for SMB/NFS mounts

**Status:** open (spike)

**Priority:** Low. The `Disk` backend only ships on macOS, where the default volumes (APFS) fully support `RENAME_SWAP` and `RENAME_EXCL`. This only affects users who materialize documents onto network mounts (SMB shares, NFS) or certain FUSE-based volumes. Those are edge-case workflows; a user hitting this sees a hard error on save, which is the correct failure mode -- the issue is purely about the quality of the error signal, not correctness or data safety.

**Symptom:** When the user's document path resolves to a filesystem that lacks `renamex_np` support (SMB mount, NFS mount, some FUSE volumes), `exchange()` and `rename_excl()` return a raw OS error with `errno == EOPNOTSUPP`. The caller (the materialize path) cannot distinguish "this filesystem fundamentally cannot do atomic renames" from a transient or permission error. The user sees a generic I/O error on save with no indication that the root cause is the filesystem type.

**Root cause:** `Disk::publish()` wraps the `renamex_np` failure with `io::Error::new(e.kind(), format!(...))`, which preserves the `io::ErrorKind` but embeds it in a generic `io::Result` return type on the `Vfs` trait. There is no `ErrUnsupported` variant or similar distinction in the Vfs error surface. The trait return type is `io::Result<T>` across the board, so the caller must match on `io::ErrorKind` -- and while `EOPNOTSUPP` does map to `io::ErrorKind::Unsupported` on Unix, the current wrapper in `publish()` constructs the error from the original `e.kind()`, which means it technically works today but is fragile and undocumented. The real gap is that no caller currently inspects the error kind to decide whether to fall back to a non-atomic path or surface a user-friendly message.

**Scope:**
- `crates/rune-vfs/src/disk.rs` -- `Disk::publish()` and `Disk::renamex_np()`. This is where `EOPNOTSUPP` first surfaces as `io::Error::last_os_error()`.
- `crates/rune-vfs/src/lib.rs` -- `Vfs` trait return types if a custom error type is introduced, or at minimum the trait-level documentation on what error kinds `exchange()` and `rename_excl()` may return.
- `crates/rune-vfs/src/mem.rs` -- `Mem` implementation if the trait signature changes; otherwise unaffected.
- Callers in `rune-core` and `rune-tui` that invoke `exchange()` / `rename_excl()` / `save_atomic()` and need to handle the unsupported case (likely by surfacing a clear message or falling back to a non-atomic write path).

**Acceptance criteria:**
- `Disk::publish()` detects `EOPNOTSUPP` (via `io::ErrorKind::Unsupported` or raw `libc::EOPNOTSUPP`) and returns a distinct, stable error that callers can match on without string parsing.
- The `Vfs` trait documents that `exchange()` and `rename_excl()` may return this unsupported error when the underlying filesystem lacks `renamex_np` support.
- The materialize caller (wherever it lives -- likely in `rune-core` or the CLI) matches on the unsupported error and either:
  - Surfaces a clear user-facing message explaining the filesystem limitation, or
  - Falls back to a non-atomic write path (durable temp + regular rename) with a footer warning about reduced crash safety.
- The `Mem` implementation either documents it never returns this error or supports injecting it via `fail_next` for test coverage.
- A test confirms the error is returned when `renamex_np` fails with `EOPNOTSUPP` (may require mocking or a controlled environment; if that is not feasible, document why and mark as a known gap).

**Notes:**
- This was called out as "Spike 4" in the original TODO. A spike is appropriate because the right answer depends on whether we want a custom error enum on the `Vfs` trait (cleaner API, broader change) or just an `io::ErrorKind` convention (simpler, stays within `io::Result`). The spike should evaluate both shapes and recommend one.
- The Go reference implementation may already handle this -- check `golang/` for how it deals with `EOPNOTSUPP` from `renamex_np` to inform the Rust decision.
- Darwin-only constraint means we only care about the macOS `renamex_np` syscall semantics; no cross-platform error mapping is needed.
- Related to CONSTITUTION §1.4.1 (atomic publish durability) and §1.4.10 (capture displaced bytes) -- if the filesystem cannot support atomic operations, the constitution's durability guarantees are weakened, and the UI should reflect that.
