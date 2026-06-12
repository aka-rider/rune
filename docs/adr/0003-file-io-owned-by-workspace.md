# File I/O moves from editor to workspace

The current editor.Model owns file load, save, rename, dirty tracking, and content hashing. Under the new architecture, textedit provides `Content() string`, `IsDirty() bool`, and `SetContent([]byte) Model` — nothing else. The workspace page owns all file I/O (LoadFileCmd, SaveFileCmd, FileRenameCmd), dirty/hash state, and file-watcher coordination.

Why: file I/O is not a text-editing concern. Three different textedit instances (main editor, title, chat input) share one workspace — the workspace is the natural owner of the file lifecycle. Removing file state from the base component keeps textedit genuinely reusable.

Consequence: markdownedit no longer emits FileSavedMsg, FileRenamedMsg, etc. It emits ContentChangedMsg when content mutates. The workspace decides when and how to persist.