# Relativize breadcrumb path against workspace root

**Status:** open
**Priority:** low — the breadcrumb renders the full absolute path (e.g., `/ Users / xiii / vault / notes / note.md`) instead of the relative path (e.g., `vault / notes / note.md`). Functional but noisy.

**Symptom:** The breadcrumb shows every component of the active document's absolute path. Go relativizes against the workspace root, showing only the portion under the workspace.

**Root cause:** `crates/rune-tui/src/breadcrumb.rs` renders every `Normal` component of the absolute path. Go's `buildCrumb` takes a `root` argument and relativizes the path against it. Rust's `App` has no workspace-root concept: `explorer.root` starts empty and is only populated after the first `^x` (Explorer toggle/load), so there's no root available at document-open time.

**Scope:**
- `crates/rune-tui/src/breadcrumb.rs` — `overlay`/`build_crumb` need a workspace root to relativize against
- `crates/rune-tui/src/app.rs` — needs a `workspace_root: PathBuf` field set at startup
- `crates/rune-cli/src/main.rs` — sets the workspace root from the initial CWD or document path

**Acceptance criteria:**
- `App` has a `workspace_root: PathBuf` field set at startup (independent of whether the Explorer pane has ever been opened).
- `breadcrumb::build_crumb` relativizes the path against `workspace_root` when the path is under the root, falling back to the absolute path when it is not.
- Draft documents (no `file_path`) continue to render nothing, as before.
- The relativization logic matches Go's `buildCrumb` behavior: string prefix replacement with separator boundary check, preserving the base name of the root directory in the relative path.
- `make build`, `make test`, and `make lint` pass.

**Notes:**
- Deliberately out of scope for the chrome-parity plan (border + breadcrumb-splice fixes only).
- Go's implementation (`golang/pkg/ui/components/breadcrumb/breadcrumb.go:56-83`) has the string prefix replacement logic with the separator boundary check (B3: `root=/a/vault` must not claim `/a/vault2/notes.md`).
