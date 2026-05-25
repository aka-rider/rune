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

All file-open intents must flow through one workspace method:

```go
func (m Model) requestOpenPath(path string) (Model, tea.Cmd)
func (m Model) requestCloseCurrent() (Model, tea.Cmd)
```

Use it for `filetree.FileSelectedMsg`, `opentabs.TabSelectedMsg`, numeric tab switching, and future open-file commands. This method owns the dirty guard check. No caller should invoke `editor.LoadFileCmd(path)` directly when switching files.

Use `requestCloseCurrent` for `CloseFile` keybinding and any future close command. No caller should close tabs or clear editor content directly when the editor is dirty.

### Dirty Guard Protocol (spec §H)

Workspace owns pending dirty-guard intent. Footer owns only the rendered guard state and key-response state.

```go
type pendingDirtyAction struct {
    kind     pendingDirtyKind // switchFile or closeFile
    path     string           // target path for switch; current path for close
    nextPath string           // closeFile only: file to load after close; empty means clear editor
    saveInFlight bool
    saveRequestID string
    expectedSavePath string
}
```

**File switch while dirty:**
1. Workspace receives `filetree.FileSelectedMsg`
2. Check `m.editor.IsDirty()` → if true, block load
3. Set footer to dirty guard mode: "Unsaved changes. [S]ave [D]iscard [Esc] Cancel"
4. On user response:
    - Save → call `m.editor.StartSave()`, store the returned `SaveIdentity` on `pendingDirtyAction`, set `saveInFlight=true`, and return the save command. Only after matching `FileSavedMsg{Path: identity.Path, RequestID: identity.RequestID, SavedContentHash: identity.ContentHash}` may workspace call `LoadFileCmd(pending.path)`.
   - Discard → `LoadFileCmd(pendingPath)` directly
   - Cancel → clear footer, stay on current file
    - Save error → keep current file/content/dirty state, clear pending save-then-load, surface error

**File close while dirty:** Close uses a separate state machine; do not reuse switch-file's save-then-load behavior.

1. Workspace receives close intent for the current file.
2. Compute `nextPath` before closing: the next active tab path after removing current, or empty if this is the last tab.
3. If `m.editor.IsDirty()` is true, set `pendingDirtyAction{kind: closeFile, path: currentPath, nextPath: nextPath}` and show footer dirty guard.
4. On user response:
    - Save → call `m.editor.StartSave()`, store the returned `SaveIdentity` on `pendingDirtyAction`, set `saveInFlight=true`, and return the save command. On matching `FileSavedMsg{Path: identity.Path, RequestID: identity.RequestID, SavedContentHash: identity.ContentHash}`, close current tab, then `LoadFileCmd(nextPath)` if non-empty, otherwise clear editor to empty clean state.
    - Discard → close current tab without saving, then `LoadFileCmd(nextPath)` if non-empty, otherwise clear editor to empty clean state.
    - Cancel → clear footer/pending action, keep current file open with dirty content intact.
    - Save error → keep current file open with dirty content intact, clear pending action, surface error.

**Save result routing order:** `editor.FileSavedMsg` and `editor.FileSaveErrorMsg` must be forwarded to `m.editor.Update(msg)` first so editor-owned save identity, saved content hash, dirty state, and errors are updated. After that update returns, workspace may evaluate `pendingDirtyAction` using the stored `SaveIdentity` and decide whether to continue open/close flow. Accumulate any command returned by `editor.Update` before returning workspace commands.

### Footer Changes

- Add `DirtyGuardResponseMsg` type (defined in footer package)
- Add dirty guard rendering state to footer model
- Footer guard mode consumes all keypresses until resolved. Only `S`, `D`, and `Esc` emit responses; all other keys are consumed as no-ops and must not reach editor/filetree/opentabs.

### OpenTabs Changes

- Add dirty indicator (e.g., `●` prefix or style change) for dirty paths
- `MarkDirty(path string) Model` / `MarkClean(path string) Model` methods

### Consumed-Key Handling

Dirty guard key routing has priority over all workspace globals and child updates. The workspace owns the pending action, so it must route guard keys before updating editor/filetree/opentabs.

Add either `m.pendingDirtyAction.Valid()` on workspace or `m.footer.InDirtyGuard()` as the guard check. On `tea.KeyPressMsg` while guard is active:

1. If `pendingDirty.saveInFlight` is true, consume every key as a no-op until the matching `FileSavedMsg` or `FileSaveErrorMsg` arrives.
2. Otherwise, update footer with the key.
3. Accumulate the footer command, if any.
4. Return immediately without forwarding the key to editor, filetree, or opentabs.
5. Handle `footer.DirtyGuardResponseMsg` in the normal message switch when the command returns.

```go
case tea.KeyPressMsg:
    if m.pendingDirty.Valid() {
        if m.pendingDirty.saveInFlight {
            return m, nil
        }
        m.footer, cmd = m.footer.Update(msg)
        return m, cmd
    }
    if m.editor.WantsModalInput() {
        m.editor, cmd = m.editor.Update(msg)
        return m, cmd
    }
```

Editor modal overlays (for example WP18 find overlay) preempt workspace globals after dirty guard priority. Focus switching, zen mode, close file, tab switching, and other workspace global keys must not run while `m.editor.WantsModalInput()` is true unless explicitly documented as an app-level emergency command.

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
- Move footer help expansion to a different physical key (e.g., `?`).
- Do not keep duplicate physical key strings and rely on context-gating. `CLAUDE.md` requires every key string to appear in exactly one binding.

This conflict must already be resolved before WP10 editing verification. WP13 verifies the guarantee at the workspace level and prevents regressions.

### Tests

- Dirty guard blocks file switch
- Dirty guard blocks all file switch routes: file tree selection, open-tabs selection, numeric tab switch, and future command-based open
- Dirty guard blocks file close routes: CloseFile keybinding and future command-based close
- Dirty close Save/Discard/Cancel/save-failure are tested with remaining tabs and with the last tab
- Save during guard → loads new file after save
- Save failure during guard → remains on current file, content intact, dirty true, error visible
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
| 4 | Global keys (focus switch: Ctrl+X/Ctrl+E, zen mode: Ctrl+O) do NOT produce text insertion in editor | User presses a workspace key and it inserts text in the document |
| 5 | Backspace in dirty guard prompt does NOT delete in editor buffer | User presses backspace to correct response → editor loses a character |
| 6 | Dirty guard save failure keeps original file open with dirty content intact | Failed save followed by load loses unsaved edits or saves wrong content |
| 7 | Dirty guard consumes printable letters, Tab, arrows, Backspace, and Enter unless they are valid guard responses | Data-loss prompt is visible but unrelated keys mutate the editor underneath |
| 8 | Every open-file route calls `requestOpenPath`; no direct `LoadFileCmd` bypass remains | Tabs or command palette bypass dirty guard and replace unsaved content |
| 9 | Every close route calls `requestCloseCurrent`; dirty Save/Discard/Cancel/save-failure paths all preserve or intentionally clear data | Closing a dirty file bypasses guard and drops unsaved content |
| 10 | Guard save completions require matching `RequestID` and expected path before open/close continuation | Unrelated in-flight save triggers pending close/open and mutates workspace state |
| 11 | `editor.WantsModalInput()` routes keys to editor before workspace globals | Find overlay is visible but focus/zen/close/tab switch changes app state underneath |
| 12 | After dirty guard Save, pressing D/Esc/printables before save completion is consumed and does not change pending action | User can race Save with Discard/Cancel and create duplicate or wrong continuation |
| 13 | Dirty guard save uses `editor.StartSave()`; workspace never calls `SaveFileCmd` directly | Guard save bypasses editor active-save tracking and valid save completions are ignored or stale ones accepted |
| 14 | Workspace forwards save result messages to editor before running dirty-guard continuation | Workspace loads/closes correctly but editor dirty/save identity remains stale |

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
- Global keys (Ctrl+X, Ctrl+E, Ctrl+O) don't insert text
- Focused-editor Tab indents instead of switching workspace focus
