package opentabs

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// TabSelectedMsg is emitted when the user explicitly selects a tab via
// keyboard navigation or mouse click within the focused opentabs component.
type TabSelectedMsg struct {
	DocID int64
	Path  string
}

// TabHandle identifies a tab by its stable VFS document identity.
type TabHandle struct {
	DocID int64
	Path  string
}

// Equal reports whether two handles name the same document.
// DocID != 0: DocID is authoritative; path is irrelevant (rename-safe).
// DocID == 0: path is the only discriminator (virtual docs: help, pre-store untitled).
func (h TabHandle) Equal(other TabHandle) bool {
	if h.DocID != 0 || other.DocID != 0 {
		return h.DocID == other.DocID
	}
	return h.Path == other.Path
}

type Tab struct {
	DocID         int64
	Path          string
	Name          string
	Pinned        bool
	Dirty         bool
	lastActiveSeq int64 // monotonic counter stamped on switch-away; 0 = never focused
}

type Model struct {
	tabs         []Tab
	cursor       int
	activeHandle TabHandle
	activitySeq  int64 // bumped each time a tab is switched away from
	width        int
	height       int
	offsetX      int
	offsetY      int
	focused      bool
	keys         keymap.Bindings
	styles       styles.Styles
}

func New(keys keymap.Bindings, st styles.Styles) Model {
	return Model{keys: keys, styles: st}
}

func (m Model) SetSize(w, h int) Model   { m.width = w; m.height = h; return m }
func (m Model) SetOffset(x, y int) Model { m.offsetX = x; m.offsetY = y; return m }
func (m Model) SetFocused(f bool) Model  { m.focused = f; return m }
func (m Model) Focused() bool            { return m.focused }
func (m Model) Cursor() int              { return m.cursor }
func (m Model) Len() int                 { return len(m.tabs) }

func (m Model) Height() int {
	if len(m.tabs) == 0 {
		return 1
	}
	return len(m.tabs) + 1
}

// PathAt returns the file path at the given tab index, or "" if out of bounds.
func (m Model) PathAt(index int) string {
	if index < 0 || index >= len(m.tabs) {
		return ""
	}
	return m.tabs[index].Path
}

// DocIDAt returns the VFS document ID at the given tab index, or 0 if OOB.
func (m Model) DocIDAt(index int) int64 {
	if index < 0 || index >= len(m.tabs) {
		return 0
	}
	return m.tabs[index].DocID
}

// AllDocIDs returns the VFS document id of every open tab that has one
// (0 excluded — the help tab is never store-backed). Lets the page batch a
// fresh, ground-truth dirty query (docstate.Store.DirtyDocs) instead of
// trusting the per-tab cached Dirty flag for a destructive decision (quit /
// evict — §1.4.8): opentabs' own flag stays render-only, the tab-bar dirty
// dot fed by the SAME query results the page already has in hand.
func (m Model) AllDocIDs() []int64 {
	var ids []int64
	for _, t := range m.tabs {
		if t.DocID != 0 {
			ids = append(ids, t.DocID)
		}
	}
	return ids
}

// SelectIndex moves the navigation cursor to the given index.
func (m Model) SelectIndex(index int) Model {
	if index < 0 || index >= len(m.tabs) {
		return m
	}
	m.cursor = index
	return m
}

// SetActive marks the tab identified by h as the active document and syncs the
// navigation cursor to it. Idempotent when h already identifies the active tab.
// Called unconditionally by workspace.finalize() so active state always mirrors
// the workspace's (docID, filePath) pair — the single golden source.
func (m Model) SetActive(h TabHandle) Model {
	if m.activeHandle.Equal(h) {
		return m
	}
	// Stamp the outgoing tab with a monotonic sequence number so EvictionCandidate
	// can order tabs by recency without needing wall-clock time.
	for i, t := range m.tabs {
		th := TabHandle{DocID: t.DocID, Path: t.Path}
		if th.Equal(m.activeHandle) {
			m.activitySeq++
			m.tabs[i].lastActiveSeq = m.activitySeq
			break
		}
	}
	m.activeHandle = h
	for i, t := range m.tabs {
		th := TabHandle{DocID: t.DocID, Path: t.Path}
		if th.Equal(h) {
			m.cursor = i
			return m
		}
	}
	return m
}

// ActiveHandle returns the (DocID, Path) handle of the currently active tab.
func (m Model) ActiveHandle() TabHandle { return m.activeHandle }

// PinIndex toggles the pinned state of the tab at index.
func (m Model) PinIndex(index int) Model {
	if index < 0 || index >= len(m.tabs) {
		return m
	}
	m.tabs[index].Pinned = !m.tabs[index].Pinned
	return m
}

// OpenFile ensures a tab exists for the given docID+path pair, adding one if
// absent. Active state and cursor are managed exclusively by SetActive (called
// from workspace.finalize) — OpenFile only manages the tab list.
// When docID != 0, the tab is keyed by docID (rename-safe).
// When docID == 0, falls back to path-keying for virtual docs (help, initial
// untitled) where no VFS identity has been assigned yet.
//
// If path already belongs to a DIFFERENT tab, that tab is DETACHED (Path
// cleared to "") first — the same disk-truth-wins, content-preserving
// reconciliation RenameFile uses (see its doc comment): path can only
// legitimately belong to one document, and this docID is the one a fresh
// Load just verified is CURRENTLY there. Without this, a rename detected via
// handleFileLoadedMsg's RenamedFrom branch (which calls RenameFile first,
// then this) would have this call silently re-introduce the exact duplicate
// RenameFile just resolved.
func (m Model) OpenFile(docID int64, path string) Model {
	detachOther := func(exceptIdx int) {
		// path=="" is never a real collision: it's the untitled sentinel
		// (§1.7 — presence/absence, not a value to compare for identity),
		// and T1 explicitly permits multiple ""-path tabs to coexist.
		// Without this guard, assigning a real docID to a FRESH untitled
		// tab (still path=="" at that point) walked every OTHER untitled
		// tab and "detached" it too — a no-op here since they're already
		// path=="", but the wrong operation for the wrong reason (found via
		// FUZZ_TRACE instrumentation, WP6 session: 27 spurious detaches in
		// one eviction-pressure run).
		if path == "" {
			return
		}
		for i := range m.tabs {
			if i != exceptIdx && m.tabs[i].Path == path {
				m.tabs[i].Path = ""
			}
		}
	}
	if docID != 0 {
		for i, t := range m.tabs {
			if t.DocID == docID {
				detachOther(i)
				m.tabs[i].Path = path
				m.tabs[i].Name = tabName(path)
				return m
			}
		}
	} else {
		for _, t := range m.tabs {
			if t.DocID == 0 && t.Path == path {
				return m
			}
		}
	}
	detachOther(-1) // -1 never matches a real index — detaches ANY tab holding path
	// Not found — add new tab.
	m.tabs = append(m.tabs, Tab{
		DocID: docID,
		Path:  path,
		Name:  tabName(path),
	})
	return m
}

// AssignDocID upgrades the docID==0 tab matching path to a real VFS docID.
// Used when the store becomes ready after the startup untitled tab was created
// store-less. No-op if no matching placeholder tab exists.
func (m Model) AssignDocID(path string, docID int64) Model {
	for i := range m.tabs {
		if m.tabs[i].DocID == 0 && m.tabs[i].Path == path {
			m.tabs[i].DocID = docID
			return m
		}
	}
	return m
}

// NameByID returns the display name of the tab with the given docID, or "".
func (m Model) NameByID(docID int64) string {
	for _, t := range m.tabs {
		if t.DocID == docID {
			return t.Name
		}
	}
	return ""
}

// HasUntitledPlaceholder reports whether a store-less untitled tab (DocID==0,
// path "") is present — the startup scratch awaiting a VFS doc once the store
// opens. False when the app was launched directly onto a file.
func (m Model) HasUntitledPlaceholder() bool {
	for _, t := range m.tabs {
		if t.DocID == 0 && t.Path == "" {
			return true
		}
	}
	return false
}

// HasTabNamed reports whether any open tab currently displays the given name.
// Used to pick the next free "Untitled N" label without touching the disk.
func (m Model) HasTabNamed(name string) bool {
	for _, t := range m.tabs {
		if t.Name == name {
			return true
		}
	}
	return false
}

// SetTabNameByID overrides the display name of the tab with the given docID.
func (m Model) SetTabNameByID(docID int64, name string) Model {
	for i := range m.tabs {
		if m.tabs[i].DocID == docID {
			m.tabs[i].Name = name
			return m
		}
	}
	return m
}

// SetTabName overrides the display name of the tab matching path.
// Prefer SetTabNameByID when the docID is known.
func (m Model) SetTabName(path, name string) Model {
	for i := range m.tabs {
		if m.tabs[i].Path == path {
			m.tabs[i].Name = name
			return m
		}
	}
	return m
}

// MarkDirtyByID sets the dirty indicator on the tab with the given docID.
func (m Model) MarkDirtyByID(docID int64) Model {
	for i := range m.tabs {
		if m.tabs[i].DocID == docID {
			m.tabs[i].Dirty = true
			break
		}
	}
	return m
}

// MarkCleanByID clears the dirty indicator on the tab with the given docID.
func (m Model) MarkCleanByID(docID int64) Model {
	for i := range m.tabs {
		if m.tabs[i].DocID == docID {
			m.tabs[i].Dirty = false
			break
		}
	}
	return m
}

// MarkClean clears the dirty indicator on the tab matching path.
// Prefer MarkCleanByID when the docID is known.
func (m Model) MarkClean(path string) Model {
	for i := range m.tabs {
		if m.tabs[i].Path == path {
			m.tabs[i].Dirty = false
			break
		}
	}
	return m
}

// HasDirty reports whether any open tab has unsaved changes.
func (m Model) HasDirty() bool {
	for _, t := range m.tabs {
		if t.Dirty {
			return true
		}
	}
	return false
}

// DirtyTabs returns handles for all tabs that have unsaved changes.
func (m Model) DirtyTabs() []TabHandle {
	var out []TabHandle
	for _, t := range m.tabs {
		if t.Dirty {
			out = append(out, TabHandle{DocID: t.DocID, Path: t.Path})
		}
	}
	return out
}

// NeighborOf returns the tab that should become active after the identified tab
// is closed, with ok=false if it would be the last tab. The tab is located by
// docID when non-zero (rename/untitled-safe, so multiple path="" untitled tabs
// resolve distinctly), else by path.
func (m Model) NeighborOf(docID int64, path string) (TabHandle, bool) {
	idx := -1
	for i, t := range m.tabs {
		if (docID != 0 && t.DocID == docID) || (docID == 0 && t.DocID == 0 && t.Path == path) {
			idx = i
			break
		}
	}
	if idx < 0 || len(m.tabs) <= 1 {
		return TabHandle{}, false
	}
	var n Tab
	if idx < len(m.tabs)-1 {
		n = m.tabs[idx+1]
	} else {
		n = m.tabs[idx-1]
	}
	return TabHandle{DocID: n.DocID, Path: n.Path}, true
}

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyPressMsg:
		if !m.focused || len(m.tabs) == 0 {
			break
		}
		switch {
		case key.Matches(msg, m.keys.Up):
			if m.cursor > 0 {
				m.cursor--
			}
		case key.Matches(msg, m.keys.Down):
			if m.cursor < len(m.tabs)-1 {
				m.cursor++
			}
		case key.Matches(msg, m.keys.PrimaryAction):
			t := m.tabs[m.cursor]
			return m, func() tea.Msg { return TabSelectedMsg{DocID: t.DocID, Path: t.Path} }
		}
	case tea.MouseClickMsg:
		return m.handleMouseClick(msg)
	}
	return m, nil
}

// handleMouseClick selects the tab under the click and asks the page to open
// it. Row 0 of the component is the "── Open ──" header; tabs start at row 1.
func (m Model) handleMouseClick(msg tea.MouseClickMsg) (Model, tea.Cmd) {
	if !m.focused || msg.Button != tea.MouseLeft {
		return m, nil
	}
	idx := msg.Y - m.offsetY - 1
	if idx < 0 || idx >= len(m.tabs) {
		return m, nil
	}
	m = m.SelectIndex(idx)
	t := m.tabs[idx]
	return m, func() tea.Msg { return TabSelectedMsg{DocID: t.DocID, Path: t.Path} }
}

func (m Model) View() string {
	var b strings.Builder

	b.WriteString(m.styles.TabsDivider.Render("── Open ──────"))

	for i, t := range m.tabs {
		b.WriteByte('\n')

		prefix := "  "
		if i == m.cursor && m.focused {
			prefix = "> "
		}
		b.WriteString(prefix)

		b.WriteString(fmt.Sprintf("%d:", (i+1)%10))
		if t.Dirty {
			b.WriteString(m.styles.TabDirty.Render("x"))
		} else {
			b.WriteByte(' ')
		}
		b.WriteByte(' ')

		name := t.Name
		if t.Pinned {
			name = m.styles.TabPinned.Render("★") + " " + name
		}

		th := TabHandle{DocID: t.DocID, Path: t.Path}
		if th.Equal(m.activeHandle) {
			b.WriteString(m.styles.TabActive.Render(name))
		} else {
			b.WriteString(m.styles.TabNormal.Render(name))
		}
	}

	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Render(b.String())
}

func tabName(path string) string {
	if path == "" {
		return "(untitled)"
	}
	parts := strings.Split(path, "/")
	name := parts[len(parts)-1]
	if name == "" {
		return path
	}
	return name
}
