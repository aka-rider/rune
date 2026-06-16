# Autosave to disk; retire the dirty flag and quit-guard

A document with a path is written to disk continuously, debounced on the freeze-at-flush cadence. There is no "unsaved changes" state: the dirty flag, the dirty indicator, and the `^C^C` quit-guard are removed. History and recovery come from the journal, the scrubber, and snapshots; `^C` flushes once and quits.

Untitled documents are the one exception — they persist as snapshots under a synthetic identity (the draft concept) but touch disk only when named or saved. ⌘S no longer "persists": it force-flushes now, and for an untitled document it triggers the naming flow.

This supersedes the plan's entire save-boundary section (`saved_revision_id`, `is_save`, revision-vs-saved dirty) and the recent dirty-flag work (`.docs/dirty-flag.md` and the quit-guard commits).

Why: with continuous autosave plus a snapshot/scrubber history, an explicit save step and an "unsaved changes" warning are friction protecting nothing — the file and the database are always within a couple of seconds of the buffer.

Consequence: the async write-tracking in `SaveIdentity` survives (autosave is still asynchronous I/O needing completion/error handling), but its dirty semantics are gone. Autosave writes trigger the file watcher against the editor's own writes; those self-writes MUST be filtered — by recording the just-written content hash, or by suppressing the watcher across the write — so the external-change/merge path does not merge a file with itself. This couples autosave to the Phase-2 watcher/merge work.
