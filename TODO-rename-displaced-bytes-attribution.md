# Audit rename displaced-bytes attribution against merge-ancestor derivation

**Status:** open
**Priority:** low — no reproducer exists; the risk is latent. Incorrect merge ancestors can silently discard real changes (§0.1 Catastrophic), but the current codebase has not exhibited this. Accepted as shipped under plan Assumption A3 during the MVP; now actionable after the branch landed.

**Symptom:** None observed. If triggered, the merge ancestor for a document would silently incorporate observations from a different document, causing the 3-way merge baseline to discard or corrupt real edits.

**Root cause:** When `rune-db` processes a rename that replaces an existing file, it files the replaced file's displaced bytes under the **renaming** document's `doc_id`. The rename test asserts `displaced.doc_id == f.ds.doc_id` (i.e., "captured under OUR doc"). The `observations` stream is the source for `newestObservation`/`ancestorAt` lookups that determine the 3-way merge baseline (§12). If an observation belonging to document B is filed under document A's stream, and later becomes A's merge ancestor, the merge will use B's content as A's baseline — silently discarding A's real changes.

**Scope:**
- `crates/rune-db/src/rename.rs` — the rename path that attributes displaced bytes.
- `crates/rune-db/src/` — any `ancestorAt` / `newestObservation` queries that read the observations stream without filtering by true document ownership.
- Merge logic in `rune-core` that consumes the ancestor — may need a guard if the DB layer cannot fully isolate.

**Acceptance criteria:**
- Audit confirms whether a rename's displaced-bytes observation can ever be returned by `ancestorAt` or `newestObservation` for a document other than the renaming one.
- If the risk is real: `ancestorAt` and `newestObservation` are scoped to exclude cross-doc-attributed observations, so a foreign file's hash never becomes another document's merge baseline.
- Regression test: bind document A, rename document B over A's path, then assert that A's `ancestorAt` / `newestObservation` remains unchanged and does not reflect B's content.
- If the risk is already impossible (e.g., the observations stream is already filtered by doc_id at query time), document the invariant with an inline comment citing why it is safe.

**Notes:**
- Recorded 2026-07-28, WP11.S5. Branch `worktree-kind-inventing-marshmallow` was fast-forward merged at `74dc1ad`.
- CONSTITUTION §12 defines the observations stream as the authoritative source for merge-ancestor lookups.
- CONSTITUTION §0.1 classifies silent content discard as Catastrophic.
- The displaced-bytes capture itself is correct per §1.4.10 (capture before discard); only the attribution to the wrong doc_id is the concern.
