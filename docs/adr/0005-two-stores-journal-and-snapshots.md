# Two stores: an in-memory journal and a permanent snapshot database

State is split across two SQLite databases, both owned by the workspace:

- An in-memory (`:memory:`) **journal** — one global, ordered timeline of every workspace interaction (edits, cursor moves, selection changes, focus changes). Continuous, ephemeral; the source for undo/redo and the scrubber. Lost on exit or crash.
- A permanent database at `~/.local/share/rune/rune.db` containing **snapshots only** — content-addressed, compressed full-document versions, plus the merge nodes that join concurrent sources. Durable across restarts.

Snapshots are written on the freeze-at-flush cadence (~2s idle, blur, close, plus a ~10s safety net during unbroken typing); the journal is written continuously in memory. The driver is `mattn/go-sqlite3` — the app already requires CGO (`pkg/merge`, `pkg/microphone`, `pkg/inputlang`), so a pure-Go driver buys nothing. Opening degrades gracefully: open the file → `mkdir -p` + create → `:memory:` with a non-blocking warning → hard fail ("internal storage failure").

This supersedes the plan's single-database design, its `operations` table and pending-ops machinery, the `modernc.org/sqlite` choice, and ADR-0007 of the plan ("SQLite in vault root"): the database is global and keyed by file identity, not vault-local.

Why: the fine-grained timeline is large, hot, and useful only within a session; snapshots are small, cold, and the only thing worth keeping across restarts. Separating them keeps the permanent database tiny and write-light and lets the journal be fast and disposable.

Consequence: nothing durable is ever uncommitted, so there is no crash-replay of pending operations — recovery is "reopen the last snapshot." Across a restart, undo is snapshot-granular only; fine-grained time travel is session-only. A hard crash loses at most the unflushed live region (≤~2s idle, ≤~10s mid-burst). Because identity is global and (from Phase 3) physical (inode+device), history follows a file across renames but does not travel with the vault to another machine.
