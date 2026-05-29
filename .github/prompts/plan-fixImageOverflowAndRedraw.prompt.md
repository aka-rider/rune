# Plan: Fix Image Rendering Overflow & Redraw in WezTerm

Fix two image rendering glitches: (1) images overflow the editor — constrain width to `m.width - 2` for a visual frame; (2) after cursor enters an image tag (collapse), ghost image pixels remain until forced redraw.

---

**Steps**

### Phase 1: Image Width Constraint (horizontal padding)

1. Add `imageMaxCols() int` helper on Model (returns `m.width - 2`, clamped ≥1) in [editor.go](pkg/ui/components/editor/editor.go) near `contentHeight()`.
2. In [commands_image_lifecycle.go](pkg/ui/components/editor/commands_image_lifecycle.go) → `discoverNewImages()` (line 24): change `maxCols := m.width` → `maxCols := m.imageMaxCols()`.
3. In [editor.go](pkg/ui/components/editor/editor.go) → `SetSize()` (line ~380): change `m.resizeImages(m.width, m.contentHeight())` → `m.resizeImages(m.imageMaxCols(), m.contentHeight())`.
4. **Left-pad the rendered image by 1 cell** so the margin is symmetric (1 left + 1 right):
   - In [view.go](pkg/ui/components/editor/view.go) → Kitty placeholder branch: prepend a single space `Cell` before the `imagePlaceholderCells(...)` slice, so the placeholder grid starts at column 1 instead of 0.
   - In [view.go](pkg/ui/components/editor/view.go) → iTerm2 space-cells branch: same — prepend a leading space cell.
   - In [commands_image_lifecycle.go](pkg/ui/components/editor/commands_image_lifecycle.go) → `replotInlineImages()` (line ~178): change `col := m.offsetX + 1` → `col := m.offsetX + 2` so the iTerm2 TTY placement also respects the 1-char left margin.

### Phase 2: Vertical overflow — no change needed

5. Already handled: `FitCells` constrains to `contentHeight()`, viewport slices exactly `contentHeight` rows, `View()` applies `MaxHeight(m.height)`. Just verify manually that footer is never obscured.

### Phase 3: Screen redraw on iTerm2 image collapse

6. Add `wasExpanded bool` field to `imageEntry` in [image_registry.go](pkg/ui/components/editor/image_registry.go).
7. After each `syncDisplay()` call, detect images that transitioned from expanded → collapsed by checking if their path no longer appears in the snapshot's image lines. Update `wasExpanded` on the entry accordingly.
8. **Return `tea.ClearScreen` (not an out-of-band TTY write) when a collapse is detected.** Bubble Tea natively handles `tea.ClearScreen` by issuing `\033[2J` and performing a full repaint on the next render cycle. This avoids the race condition where an async TTY write of blank spaces would execute _after_ Bubble Tea has already redrawn valid text into those cells.
9. In the cursor-movement / edit path in [apply.go](pkg/ui/components/editor/apply.go) or [editor.go](pkg/ui/components/editor/editor.go)'s Update, after `syncDisplay()` + `scrollToCursor()`: if any image's `wasExpanded` transitioned to false (collapsed), append `tea.ClearScreen` to the returned `tea.Cmd` batch and reset the flag.

**Why `tea.ClearScreen` instead of `ClearImageAreaCmd`:** iTerm2/WezTerm images are placed out-of-band via direct TTY writes. Bubble Tea's renderer is unaware of those pixels. A `tea.ClearScreen` tells Bubble Tea to wipe the entire terminal and repaint from scratch — the only reliable way to remove ghost pixels without race conditions. The cost (one full repaint) is negligible since collapse is an infrequent user action (cursor entering an image tag).

### Phase 4: Scrolling — no change needed

10. `scrollToCursor()` already recomputes display rows against the current (potentially collapsed) snapshot. Verify via manual testing.

---

**Relevant files**

- `pkg/ui/components/editor/editor.go` — add `imageMaxCols()` helper; fix `resizeImages` call in `SetSize()`
- `pkg/ui/components/editor/commands_image_lifecycle.go` — change `maxCols` in `discoverNewImages()`; shift iTerm2 placement col in `replotInlineImages()`; manage `wasExpanded` tracking
- `pkg/ui/components/editor/image_registry.go` — add `wasExpanded` field to `imageEntry`
- `pkg/ui/components/editor/view.go` — prepend 1-cell left padding before image placeholder/space cells
- `pkg/ui/components/editor/apply.go` — emit `tea.ClearScreen` on collapse detection

**Verification**

1. `go build ./...` — no compile errors
2. `go test ./pkg/ui/components/editor/... ./pkg/imagekit/...` — pass
3. Manual in WezTerm: wide image → verify 1-char margin on each side
4. Manual: cursor into image tag → verify clean disappearance (no ghost pixels)
5. Manual: cursor out of tag → verify image re-appears at correct size
6. Manual: scroll with images → footer always visible on top

**Decisions**

- 1-char padding each side (`width - 2`) as the image width constraint, with left padding applied at render time (both View cell output and iTerm2 TTY placement offset)
- Vertical overflow needs no code change (existing clamping is correct)
- Ghost pixels on collapse cleared via `tea.ClearScreen` — a framework-native full repaint that avoids race conditions with out-of-band TTY writes
- Kitty path likely doesn't need this fix (placeholder cells vanish from View output → terminal should GC), but worth verifying separately

**Further Considerations**

1. Does Kitty also exhibit ghost pixels on collapse? If so, emit `EncodeDelete` on collapse for Kitty too.
2. When an animated GIF collapses, verify `armImageTicks` properly stops (it checks `imageVisible` which scans the snapshot — should work).
3. Consider whether `wasExpanded` tracking should live on the entry vs. as a separate set computed by diffing snapshots — the diff approach avoids polluting the registry struct.
