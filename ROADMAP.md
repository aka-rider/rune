
- [x] Scaffolding: Done!.
- [x] Pre-flight: Port fuzzing infra and start fuzzing
- [x] Add sqlite, state saving, models, VFS
- [x] Add Explorer, Open Tabs and Footer 
- [ ] Architecture with three-sitter, config, themes
- [ ] 100% features parity with golang's **markdown-edit** TUI parity with rune
	- rendering readonly view and full editor r's keystrokes,
  row movements, etc. Rendering parity: colors, styles. Verification via ttyd golang/rust on the same file
  
- Add links and links traveling, wiki-links support, HTTP decoding
- keystrokes / chords 1:1

5. Markdown (+HTML) rendering capabilities:
  - images
  - tables 1:1 rune's behavior
6. Future: tree sitter integration
6.1 Recognize file type + render icon for: JavaScript,
C++, TypeScript, Markdown, etc. (copy NeoVim's / Helix /
best-in-breed icons pack)
7. Merge external changes (built-in 3-way merge)
8. What else?



 ENDGAME: rune in rust
  1. Scaffold. Mimics the rune editor `rune(in rust)
  <markdown file>` main Keystrokes, clipboard; Markdown
  rendering: basic markdown preview + editing combined
  1.1 Add links and links traveling, wiki-links support,
  HTTP decoding
  2. Add sqlite, state saving, models, VFS
  3. keystrokes / chords 1:1
  4. Add Explorer, Open Tabs and Footer, TUI parity with
  rune
  4.1 Port fuzzing infra and start fuzzing, main focus on
  improving human session
  5. Markdown (+HTML) rendering capabilities:
      - images
      - tables 1:1 rune's behavior
  6. Future: tree sitter integration
  6.1 Recognize file type + render icon for: JavaScript,
  C++, TypeScript, Markdown, etc. (copy NeoVim's / Helix /
