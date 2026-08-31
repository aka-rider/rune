<h1 align="center">rune — Markdown Editor for macOS terminal 📎</h1>

<p align="center"><img src="assets/rune-intro.gif" alt="Rune into screencast" width="80%" /></p>

---

## Good News Everyone

- **MacOS-native** look and feel with familiar **⌘** - combinations
- **Live Markdown Rendering** — Bold, italic, headings, blockquotes, code fences with real tree-sitter syntax highlighting across 32 languages, tables with adaptive layouts, task lists, thematic breaks, YAML frontmatter, `[[wikilinks]]`, and setext headings.
- **Task lists** - `- [x] buy milk` syntax
- **Inline Images** — Render PNG, JPEG, GIF, WebP, BMP, TIFF, and SVG directly in your terminal via the Kitty graphics protocol. GIFs render as a still frame.
- **Mouse Support** — Click to focus, drag to select, drag pane dividers, scroll through files, multi-click select.
- **Obsidian Vault Compatible** — Launch Rune from the vault root so `[[wikilinks]]` and embeds resolve across your notes. Some Obsidian-flavored constructs (callouts, `==highlights==`, `#tags`, math) render as plain text for now.
- **Multi-Cursor Editing** — Add cursors above or below the current line.
- **Crash Recovery** — Every edit lands in a durable journal with periodic snapshots; unsaved work survives a crash.
- **Conflict Guard + Merge** — An external edit to the open file is caught before it can be overwritten, with a built-in 3-way merge resolver (`^M`) to reconcile both sides.
- **Reading View** — Toggle a read-only rendered view with `^⇧P`/`⌘⇧P`.
- **Tabs + File Explorer** — Type-to-search jumps straight to a file.

### File Explorer

quick search, just start typing

<p align="center"><img src="assets/rune-explorer.gif" alt="Rune file explorer screencast" width="80%" /></p>

### Table rendering

<p align="center"><img src="assets/rune-table-rendering.gif" alt="Rune Obsidian-style task list screencast" width="80%" /></p>

### Task list

<p align="center"><img src="assets/rune-task-list.gif" alt="Rune Obsidian-style task list screencast" width="80%" /></p>
 
 
---

## Installation

Prebuilt binaries cover macOS (Apple Silicon) and Linux (x86_64/aarch64).

### Homebrew

```sh
brew tap aka-rider/tap
brew trust aka-rider/tap
brew install aka-rider/tap/rune
```

Upgrading from the old cask? First run `brew uninstall --cask rune-edit`.

### Nix

```sh
nix run github:aka-rider/rune        # try it
nix profile install github:aka-rider/rune   # install it
```

The flake builds from source at any tag, so it tracks releases with no extra publish step.

---

## Keybindings

MacOS-native ⌘+c/v/z, and others should work.
Sometimes, terminal emulator intercepts these combinations. 
Either configure the terminal emulator, or use fallback: ⌘⇧+... or Ctrl+...

For example, select all: ⌘+a, ⌘⇧+a, ^a

Some keybindings may not work with non-English input sources.
For instance, rune receives ⌘+м instead of ⌘+v (Ukrainian keyboard), this is a terminal limitation.

Press `F1` inside Rune for the list of keyboard shortcuts — the help page is generated live from the keymap, so it never drifts.


## Recommended Terminals

| Terminal | Notes |
|----------|-------|
| [Ghostty](https://ghostty.org/) | Focused on compatibility with VT standards |

Kitty, iTerm2, WezTerm work too, with all kinds of bugs. 
rune relies on terminal protocol extensions (super key, image rendering, clipboard, etc.).
Inline image rendering needs a Kitty-graphics-protocol terminal (Kitty, Ghostty); other terminals fall back to an info card.

## Credits

- [ratatui](https://github.com/ratatui/ratatui)
- [comrak](https://github.com/kivikakk/comrak)
- [tree-sitter](https://github.com/tree-sitter/tree-sitter) and its grammars

---

## [MIT License](LICENSE.txt)
