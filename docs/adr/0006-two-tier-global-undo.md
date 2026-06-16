# Undo is a two-tier global timeline: comfortable ⌘Z and a time-travel scrubber

Undo/redo operates on one global, linear timeline spanning all three surfaces (main editor, title, chat) and their focus changes. It has two tiers:

- **⌘Z / ⌘⇧Z** step at edit granularity. One press inverts the last edit anywhere in the workspace, restoring the cursor/selection/focus context around it and pulling focus to that surface. Comfortable: navigation events are context, not separate stops.
- A **scrubber** UI walks the full event journal (and snapshots) to reconstruct and preview any past state — the literal time machine, for when edit-granular stepping is too coarse.

This supersedes ADR-0002 of the plan (branch-aware undo). Because undo now lives in the in-memory journal rather than the permanent graph, the entire branch-aware apparatus — per-source undo traversal, LCA-for-undo, the `source='local'` filter — is removed. The permanent store keeps a DAG only to represent merges from concurrent sources (external edits, agents), never for undo.

Why: "time travel" and "comfortable" are in tension. Edit-granular ⌘Z keeps the common case calm; a separate scrubber gives true event-by-event travel without making every cursor twitch an undo stop.

Consequence: ⌘Z can move focus between panes — it lands wherever the last edit was. ⌘Z stays cheap (invert one event); only the scrubber's far jumps reconstruct from nearest snapshot + replay. The scrubber is a new component (Phase 2+). Whether external/agent changes appear in the ⌘Z timeline is deferred — the current lean is no (they are recorded as snapshots, outside ⌘Z, but shown as scrubber markers).
