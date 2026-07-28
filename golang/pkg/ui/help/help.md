# Rune Help

Rune is a terminal markdown editor and note-taking app. The workspace has
three panes:

- **Explorer** (`^x`) — browse and open files in the current directory.
- **Editor** (`^e`) — edit the open document. This is where you usually work.
- **Chat** (`^r`) — ask Rune about the open file.

Open files appear as tabs; switch between them with `^1`–`^9` and close the
current tab with `^w`. This help page is itself a read-only tab — press `^w`
to close it and return to editing.

## Voice input

Press `^v` to start and stop dictation. Rune transcribes speech directly into
the focused editor using a local whisper.cpp server, so your audio never
leaves your machine.

## Obsidian vaults

Rune understands Obsidian-style vaults. Wiki links written as `[[note]]`
resolve against the markdown files in your workspace; follow one by clicking
it to open the linked note.

## Keyboard shortcuts

<!-- KEYBINDINGS -->
