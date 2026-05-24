# WP13 — Workspace Integration & Data-Loss Guards

## Scope

`pkg/ui/pages/workspace/`, `pkg/ui/components/footer/`, `pkg/ui/components/opentabs/`

## Dependencies

- WP8 (editor component with messages and query methods)

## Deliverables

### Workspace Message Routing

- Route `editor.ContentChangedMsg` to opentabs (dirty/clean indicator on tab)
- Query `editor.CursorInfo()` directly after editor.Update returns, set footer state (§5.4 compliant — no Cmd for sync state)
- Forward `editor.FileLoadedMsg` to opentabs for tab tracking

### Dirty Guard Protocol (spec §H)

**File switch while dirty:**
1. Workspace receives `filetree.FileSelectedMsg`
2. Check `m.editor.IsDirty()` → if true, block load
3. Set footer to dirty guard mode: "Unsaved changes. [S]ave [D]iscard [Esc] Cancel"
4. On user response:
   - Save → `SaveFileCmd`, then on `FileSavedMsg` → `LoadFileCmd(pendingPath)`
   - Discard → `LoadFileCmd(pendingPath)` directly
   - Cancel → clear footer, stay on current file

**File close while dirty:** Same protocol. If discard, editor clears to empty state.

### Footer Changes

- Add `DirtyGuardResponseMsg` type (defined in footer package)
- Add dirty guard rendering state to footer model
- Footer handles S/D/Esc keys when in guard mode

### OpenTabs Changes

- Add dirty indicator (e.g., `●` prefix or style change) for dirty paths
- `MarkDirty(path string) Model` / `MarkClean(path string) Model` methods

### Consumed-Key Handling

Global keys handled by workspace (focus switch, zen mode, tab switch) MUST NOT be forwarded to editor as text input:

```go
case tea.KeyPressMsg:
    // Handle global keys FIRST
    switch {
    case key.Matches(msg, m.keys.FocusExplorer):
        // ... handle, return early or set consumed flag
    }
    // Only forward to editor if not consumed
    if !consumed {
        m.editor, cmd = m.editor.Update(msg)
    }
```

### Backspace Conflict Resolution

Current `pkg/ui/keymap/keymap.go` binds backspace to footer help expansion. Resolution:
- Move footer help expansion to a different key (e.g., `?` when footer focused)
- OR context-gate: backspace only triggers help when footer is focused, never when editor is focused

### Tests

- Dirty guard blocks file switch
- Save during guard → loads new file after save
- Discard during guard → loads new file immediately
- Cancel → stays on current file
- Global keys don't produce text in editor
- ContentChangedMsg routes to opentabs

## Constraints

- CLAUDE.md §2.4: messages defined in producer's package
- CLAUDE.md §5.4: no Cmd for synchronous state (use query methods)
- CLAUDE.md §3.2: chord/guard state in footer (renders the prompt)
- CLAUDE.md §3.3: focus-scoped key handling
- Under 500 LoC per file

## QA Gates

These gates are the last line of defense against data loss at the application level.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | File switch with unsaved changes → dirty guard fires (switch blocked until user responds) | User accidentally switches file → unsaved work silently discarded |
| 2 | Dirty guard "Save" path: save completes, THEN new file loads (not before) | File loads before save completes → save writes wrong content (new file’s content) |
| 3 | Dirty guard "Cancel" → stays on current file with all content intact | Cancel somehow clears buffer or resets state |
| 4 | Global keys (focus switch: Tab/Ctrl+E, zen mode: Esc) do NOT produce text insertion in editor | User presses Tab to switch pane → tab character inserted in document |
| 5 | Backspace in dirty guard prompt does NOT delete in editor buffer | User presses backspace to correct response → editor loses a character |

**Testing approach:** Integration tests simulating message sequences (file select → dirty check → guard response → verify state). Gate 4-5 via key-event injection + content assertion.

## Verification

```bash
go test ./pkg/ui/pages/workspace/ -v
go build ./...
```

Manual verification:
- Open file, edit, try switching file → guard appears
- Press S → saves then switches
- Press D → switches without save
- Press Esc → stays
- Global keys (Tab, Ctrl+E) don't insert text
