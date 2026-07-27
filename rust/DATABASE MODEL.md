
The HUMAN: >
    rust sqlite multiprocess stack.
    Use-case: one global storage for tens of instances of the same editor

For "one shared DB file, tens of OS processes on the same machine," plain SQLite in WAL mode is the right answer — but the multiprocess part lives almost entirely in the pragmas and transaction discipline, not the crate choice.

## Stack

- **`rusqlite`** (currently 0.40.x) with the `bundled` feature. Direct, synchronous, gives you full control over pragmas and busy handlers. The `bundled` feature matters here: it guarantees every instance links the *same* SQLite build, so you're not debugging version skew between a system libsqlite3 and a vendored one.
- **`rusqlite_migration`** or `refinery` for schema versioning.
- **`deadpool-sqlite`** or `tokio-rusqlite` if your editor is async; otherwise a dedicated DB thread with a channel. Either way, never touch SQLite from the UI thread.
- **`directories`** to place the file in a machine-local dir (`~/.local/share`, `%LOCALAPPDATA`, `~/Library/Application Support`) — not roaming, not iCloud-synced, not a network home.

`sqlx` works too if you want compile-time query checking, but you'll be configuring the same pragmas via `SqliteConnectOptions` and fighting its pool abstraction for the single-writer pattern. 
`diesel` is fine if you want the ORM. Skip Turso Database (the Rust rewrite, formerly Limbo) — its maintainers still state it isn't ready for production use, and its MVCC concurrency story isn't yet the thing you'd bet a shared user-data file on. libSQL is production-ready but buys you nothing for a purely local multiprocess case.

## Connection setup

```rust
fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;

    // FIRST — switching journal mode takes a lock; without this,
    // two instances starting simultaneously will collide.
    conn.busy_timeout(Duration::from_secs(5))?;

    // journal_mode returns a row, so query it rather than execute it.
    let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    debug_assert_eq!(mode, "wal");

    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_size_limit", 64 * 1024 * 1024)?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000)?;
    conn.pragma_update(None, "mmap_size", 256 * 1024 * 1024)?;
    Ok(conn)
}
```

`journal_mode` is persistent in the file; the rest are per-connection and must be set on every open. `synchronous=NORMAL` under WAL means you can lose the last few commits on power loss but the file stays consistent — the right trade for editor state. Never set `locking_mode=EXCLUSIVE`; it kills multiprocess access.

## Write discipline — the part that actually bites

SQLite allows one writer at a time. With N processes, the failure you'll hit is *not* the one `busy_timeout` saves you from:

**Always `BEGIN IMMEDIATE` for anything that will write.** A deferred transaction that reads first, then writes, must upgrade its lock. If another process wrote in between, you get `SQLITE_BUSY_SNAPSHOT`, and the busy handler is *not* invoked for it — the transaction is dead and must be rolled back and retried from scratch. `BEGIN IMMEDIATE` takes the write lock up front, where the busy handler does apply.

```rust
let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
```

Wrap that in a retry loop with jittered backoff for `SQLITE_BUSY`/`SQLITE_BUSY_SNAPSHOT`, cap at ~5 attempts, and keep write transactions to milliseconds. Never hold a write transaction open across a user interaction or an fsync of an actual file.

## Cross-process change notification

SQLite has no cross-process notification. `update_hook` only fires for your own connection. Options, roughly in order of how much I'd reach for them:

**`PRAGMA data_version`** — returns an integer that changes whenever *another* connection commits. Cheap enough to poll on a 200ms–1s timer from a background thread; when it moves, invalidate caches and re-query.

```rust
let v: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
```

**File watching on the `-wal` file** via `notify`, used as an interrupt to trigger a `data_version` check rather than as a source of truth. Cuts latency to near-zero without the polling cost; still fall back to a slow poll because watchers miss events.

**A changelog table** (`(seq INTEGER PRIMARY KEY, kind TEXT, entity_id, at)`) if you need to know *what* changed, not just *that* something did. Each process tracks its last-seen `seq`, prunes old rows periodically. Combine with the `data_version` poll as the trigger.

If you find yourself needing sub-100ms propagation with ordering guarantees, that's the signal to move to a daemon (below).

## Migrations with N processes racing

Tens of instances can launch at once — e.g. session restore after reboot. The migration path must be inside a single `BEGIN IMMEDIATE`: read `PRAGMA user_version`, decide, migrate, bump, commit. Losers of the race then see the new version and no-op. Don't check the version outside the transaction.

The nastier problem is **binary version skew**: the user updates the editor while five windows from the old build are still running. Old instances now face a newer schema. Mitigations, pick one:

- Additive-only migrations (new tables/columns, never drop or retype), plus `SELECT` explicit column lists everywhere. Old code keeps working.
- Store an `app_schema_version` and have instances that find a newer one degrade to read-only with a "restart to continue" banner.

## Other things that will surprise you

- **WAL requires shared memory** (`-shm`, mmapped). It does not work on NFS/SMB. If a Windows user's `%APPDATA%` is a redirected network share, WAL silently fails to engage or errors. Use `%LOCALAPPDATA%` and check the `journal_mode` query actually returned `wal`; fall back to `TRUNCATE` journaling if not.
- **Checkpoint starvation.** With many long-lived readers, checkpoints never find a quiet moment and the WAL grows unbounded. `journal_size_limit` caps it after a successful checkpoint, but also run `PRAGMA wal_checkpoint(TRUNCATE)` from whichever instance is last to exit, and consider a `PASSIVE` checkpoint on an idle timer.
- **Split hot from shared.** Editor session/window/cursor state gets written constantly and nobody else needs it — keep that in a per-instance file. The shared DB should hold recent files, projects, settings, and any index: reads frequent, writes rare. This is the single biggest lever on contention.
- **Cross-database transactions in WAL mode are not atomic.** If you `ATTACH`, don't assume a commit spanning both files is all-or-nothing.
- **Read connections should be separate from the write connection.** One writer connection plus a small read pool per process; WAL readers don't block the writer.
- `PRAGMA optimize` on clean shutdown keeps the query planner honest over time.

## When to go daemon instead

The direct-shared-file design has no lifecycle problems, survives any process crashing, and needs no IPC — that's a lot of value. Switch to a single owner process with a Unix socket / named pipe only if you hit one of: write contention that retry loops can't hide (a shared full-text index being rebuilt is the usual culprit), a need for real push notifications with ordering, or coordination that isn't naturally expressible as database state. Then you inherit spawn races, upgrade handoff, and crash recovery — so make it a considered move, not the default.

THE HUMAN: >
    search the Internet for the best in class library that does just that - 
    rust API over ACID WAL Durable sqlite.
    If you can't find anything robust and reliable, say so.

## Verdict: no. Nothing in the Rust ecosystem does this well.

I searched crates.io, lib.rs, and docs.rs for a crate that packages "correct multiprocess ACID SQLite" behind an API. There isn't one that I'd call robust. The closest candidates each fail on the specific requirement, and the failures are informative:

**`sqlite-watcher`** (~416k downloads, v0.7.0, updated ~2 months ago) is the best-maintained thing in this space and would be the obvious pick for change notification — except its own docs disqualify it: the Watcher can be used with one or multiple connections, but the model only works for connections inhabiting the same process space; while SQLite supports being modified by multiple processes, the current observation does not support that use case. That's exactly your requirement, explicitly out of scope.

**`sqlite-rwc`** ("Reader Writer Concurrency Setup for Sqlite3") is the closest in spirit and clearly written by someone who understands the problem — it maintains a pool of read-only connections plus one write connection, enforces exclusive writer access at the pool level rather than relying on SQLite's sleep-retry loop, and makes all other connections read-only so you can't accidentally write outside the designated writer. It even names your exact situation: by enforcing exclusive access at this level, the only remaining SQLITE_BUSY errors come from another process. But it has **2,649 all-time downloads** and sits at v0.4.0. That's a personal project, not a dependency for your users' data.

**`rusqlite`** (84.5M downloads) is the substrate everything else is built on, and it's excellent — but it deliberately gives you primitives, not policy. Worth noting it has quietly improved here: newly created connections now default to a busy_timeout handler of 5000ms, although the docs flag this as subject to change.

**`sqlx`** is arguably the most multiprocess-aware of the mainstream options in one narrow respect: it does not set a journal mode by default, specifically to avoid unintentionally flipping a database into or out of WAL mode, and warns that opening a WAL database with a different journal_mode erases the setting and requires an exclusive lock, which can produce SQLITE_BUSY if another connection is active. Good instinct, but it's a driver, not a coordination layer.

Ruled out for different reasons: **`hiqlite`** solves cross-*machine* HA via a Raft cluster on top of rusqlite and openraft — enormously heavier than you need and the wrong axis entirely. **Turso Database** remains under heavy development and not ready for production per its own maintainers. **`FrankenSQLite`** advertises MVCC concurrent writers, but its own docs state that multi-process swarm-write is "under active hardening," tracked in issue #70 — i.e. the exact feature you'd adopt it for is unfinished. **`async-sqlite`** (~230k downloads) is just a thread-offload wrapper.

I also checked what a comparable editor does: Zed's `sqlez` is a type-safe binding layer wrapping a ThreadSafeConnection, with migrations declared per-Domain — but it's an internal workspace crate, unpublished, and Zed is single-process/multi-window, so it never had to solve your problem either. Other ecosystems aren't better; Python's `msqlite` is the same gap filled by locking the DB file on every access with a retry loop, which is a sledgehammer, not a design.

## Why the gap exists

SQLite already *is* the best-in-class ACID/WAL/durable engine, and rusqlite already exposes it faithfully. What's missing isn't a storage layer — it's a **policy layer**: transaction-behavior discipline, retry classification, migration race arbitration, checkpoint ownership, and cross-process invalidation. Every one of those has app-specific answers (how long to block the UI, whether stale reads are tolerable, what to do about version skew), which is precisely why nobody has shipped a general-purpose crate. The SQLite forum's own recommendation for your notification problem is still the hand-rolled one: write triggers that log changes to a table, then have other processes poll data_version periodically and read the log when it moves.

## What I'd actually do

Write a ~400-line internal module over `rusqlite` — open/pragma policy, an `Immediate`-only write path with a retry classifier, a `data_version` poller, and migration under `BEGIN IMMEDIATE`. That's a day or two of work and you own the semantics.

Two things worth borrowing rather than inventing: read `sqlite-rwc`'s source for the reader-pool/single-writer structure (it's small enough to read in an hour, and it's a sound design even if you don't depend on it), and take `sqlite-watcher`'s `State`/`TableObserver` split as the shape for your notification API — it's designed to be [patched into an existing connection](https://docs.rs/sqlite-watcher/latest/sqlite_watcher/), so its in-process machinery can sit behind your cross-process trigger without redesign if you later want per-table granularity.

/architect ENDGAME:>
    rust rune-db crate abstracting ACID WAL sqlite as a tailor-made micro-orm modeling
        VFS primitives, Buffer, Edits, Journal, etc...
    rune-vfs crate as thin layer over filesystem -> rune-db cache -> disk -> rune-db cache refresh -> caller
    underneath:
        1x Reader 1x Writer thread.
        API: request -> determine thread status -> return error status immediatelly -> enqueue operation -> return Posted
        STRICT DISCIPLINE: read/write operations separation
        Priorities: 
            1. ROBUSTNESS, ZERO DATA LOSS, ZERO DATA CORRUPTION
            2. Simplicity
            3. Sytem Footprint. NO HAMMERING SSD!.
                    For example: Explicit checkpoints: flush to the database every second or so, BUT should user perform massive change: "Select All"; "Delete" -> explicit checkpoint
                        -> explicit checkpoint shall be automated: if changes > your usual typing, mild editing -> force checkpoint
        