# Plan: Rune TUI Wireframe & Architecture

## TL;DR
Design wireframe for a 3-pane TUI markdown editor (file tree | live-preview editor | AI chat) with unified command bus, voice control, and LLM-first interactions.

## Final Wireframe

```
┌──────────────────┬──────────────────────────────────────────────┬──────────────────────────────┐
│ Files (~/notes)  │ notes/ > daily/ > 2026-05-21.md              │ Thread: 2026-05-21.md        │
│                  │                                              │                              │
│ 2026-05-21.md ● │ # Meeting Notes — 2026-05-21                 │                              │
│ 2026-05-20.md   │                                              │   /search action items from  │
│ ideas.md        │ ## Attendees                                  │   last week                  │
│ draft.md        │ - **Alice**                                   │                              │
│ projects/       │ - Bob                                         │ Found 3 notes with action    │
│                  │                                              │ items:                       │
│                  │ ## Action Items                               │                              │
│                  │ ☐ Ship v0.1                                   │ ⏺ Search agent finished      │
│                  │ ☑ Draft wireframe                             │   ├ 2026-05-14.md            │
│                  │                                              │   │ "Fix deploy pipeline"     │
│                  │ > **Note:** Blockquotes render                │   ├ 2026-05-15.md            │
│                  │ > with a visible left bar                     │   └ rune.md                  │
│                  │                                              │     "Finish UI wireframe"   │
│                  │ ```go                                         │                              │
│                  │ func main() {                                 │ I can insert these into your │
│                  │     fmt.Println("hi")█                        │ current note as a list, or   │
│                  │ }                                             │ open each file. What would   │
│ ── Open ──────  │ ```                                           │ you prefer?                  │
│ ★ rune.md       │                                              │                              │
│ ★ ideas.md      │                                              │────────────────────────────── │
│   draft.md      │                                              │ ❯ insert them as a table     │
│                  │                                              │                              │
│                  │                                              │                              │
├──────────────────┴──────────────────────────────────────────────┴──────────────────────────────┤
│ [/] Command  [^W] Close  [^1-3] Focus  [Esc] Zen        Ln 8, Col 12   W:142   🎤 Dictate    │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘
```

## Layout Specification

### Left Pane — File Tree + Tabs
- **Top section**: Flat file list (current directory only, no recursive tree)
- **Bottom section**: "Open" — vertical tab bar
  - ★ pinned files always float to top
  - Other open files listed below
  - ● indicator for active/unsaved
- Collapsible (toggle with ^1)

### Center Pane — Editor
- **Breadcrumb** bar at top: `dir/ > subdir/ > file.md`
- **Live inline preview** (Obsidian-style): bold renders as bold, checkboxes render, code blocks get syntax highlighting — all while editing
- Word wrap ON by default (toggleable via options)
- No separate source/preview split — single unified view
- Cursor is standard block/line; changes to 🎤 glyph when voice dictation active

### Right Pane — AI Chat & Command Centre
- **Thread indicator** at top: auto-switches to per-note thread when active note changes
  - Multiple threads: one per note + one global
  - Auto-switch on note change
- **Conversation area**: scrollable
  - User messages: right-aligned, dimmed/less contrast
  - Assistant messages: left-aligned, full contrast
  - Agent work: tree-style collapse (⏺ ├ └ pattern, like Claude Code)
  - NO borders/boxes around messages
  - Word wrap always on
- **Input prompt**: always visible at bottom
  - `❯` prompt character
  - Grows vertically on content overflow (variable height)
  - Slash commands typed here only (not in editor)
  - `/` triggers fuzzy command palette overlay anchored to right pane area
- Collapsible (toggle with ^3)

### Footer
- Contextual keyboard shortcuts (change based on focused pane)
- Position info: `Ln X, Col Y`
- Stats: `W:NNN` (word count)
- Voice mode indicator: `🎤 Dictate` / `🎤 Command` / `🎤 Off`
- Always visible (even in zen mode? TBD)

### Zen Mode
- `Esc` toggles zen mode: editor only, no side panes, no chrome
- Footer may remain for minimal context

## Architectural Decisions

### 1. Unified Command Bus (CRITICAL)
Every user action routes through a single command dispatch system:
- Keystrokes → Command
- Slash commands (typed in chat `❯`) → Command
- Voice-transcribed commands → parse → Command
- LLM tool-use invocations → Command

This means:
- One `Command` type/interface handles all interactions
- No separate codepaths for UI-triggered vs AI-triggered vs voice-triggered
- All editor operations (insert, navigate, format, search, open file) are commands
- The LLM can invoke any command the user can
- Slash fuzzy palette shows all registered commands

### 2. Voice Control — Two Modes
1. **Dictation mode**: Speech-to-text streams to cursor position (works in editor AND chat input)
2. **Command mode**: Focuses chat input, transcribed text parsed as commands

Visual feedback: cursor glyph changes to 🎤, transcription appears in-place at cursor.

### 3. Chat IS Search
- No separate search panel/pane
- Search is a slash command (`/search`, `/find`, etc.) like any other
- Results appear in chat conversation as navigable links
- Same unified command bus

### 4. Live Preview Only
- Single editor mode: live inline rendering (like Obsidian)
- No side-by-side preview split
- No pure-source mode (markdown formatting visible but rendered inline)

### 5. No Inline AI Completions
- No ghost-text / copilot-style suggestions in editor
- All AI interaction through explicit chat pane commands
- AI can modify the document only when explicitly asked via command

### 6. Flat File Navigation
- File tree shows current directory only (flat list)
- Directory entries shown but navigation is flat (enter dir = new flat view)
- Fuzzy file finding via slash command, not tree traversal

## Scope Boundaries

### In Scope
- 3-pane layout (tree | editor | chat)
- Live inline markdown preview
- Unified command bus architecture
- Voice dictation + voice commands
- Per-note + global AI threads
- Vertical tab bar (pinned + open)
- Breadcrumb path bar
- Contextual footer
- Zen mode
- Togglable panes

### Explicitly Out of Scope
- Backlinks / graph view
- Tags / metadata panel
- Separate search results panel
- Git status indicators
- Notification area
- Horizontal tab bar
- Inline AI completions / ghost text
- Recursive file tree
