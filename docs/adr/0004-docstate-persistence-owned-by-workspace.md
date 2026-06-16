# docstate persistence is owned by the workspace, not the editor

Undo/redo and durable history are owned by the workspace, not by textedit. textedit keeps only its buffer, cursors, and the primitives to apply and revert edits and set cursors; it has no undo stack of its own. The workspace owns the persistence layer (docstate) and the undo/redo journal, and drives every textedit instance — main editor, title, chat input — through those primitives.

This supersedes the persistent-document-state plan's "the editor owns `docstate.Document`". There is no `editor` package — it is textedit + markdownedit — and ADR-0003 already places file I/O and identity on the workspace. Putting a persistence-backed document on the base component would have given the title bar and chat input their own history and coupled the most reusable component in the app to storage.

Why: persistence and a whole-workspace undo timeline are not base text-editing concerns. Keeping textedit storage-free preserves its reusability (ADR-0003) and keeps SQLite off the keystroke hot path — the workspace observes edits and cursor/selection changes and records them into the journal.

Consequence: textedit loses its `history.UndoStack` and its undo/redo commands; the coalescing and inverse logic moves up into the workspace journal. The workspace gains a seam to observe each applied edit plus cursor/selection/focus changes, and to set content/cursors when driving undo. Standalone textedit (outside the workspace) has no undo; the workspace supplies it. The journal spans focus changes, so it is inherently workspace-level — only the workspace sees focus move between panes.
