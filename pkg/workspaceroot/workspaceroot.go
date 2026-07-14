// Package workspaceroot resolves rune's workspace root (file tree root +
// docstate location) at launch when the user did not pass -w. It is a pure
// function of (cwd, home) plus whatever the injected vfs.FS reports — no
// os.*, no interactive code (§1.4.9) — so the whole state machine is
// unit-testable against vfs.Mem without touching a real disk.
//
// The only silent outcome besides -w is finding an existing .rune/ directory
// walking up from cwd. Every other case returns a Prompt so the user
// consents to where a NEW .rune/ gets created — see cmd/rune/rootchooser.go
// for the UI that consumes it.
package workspaceroot

import (
	"io/fs"
	"path/filepath"
	"strings"

	"rune/pkg/vfs"
)

// Marker names scanned for at each directory level. A marker counts whether
// it is a directory OR a file — git worktrees/submodules use a .git *file*,
// so detection is by name only, never fs.DirEntry.IsDir(). Single named
// constant set so adding a future marker (Logseq, .zk, ...) is one line.
const (
	markerRune     = ".rune"
	markerGit      = ".git"
	markerObsidian = ".obsidian"
)

// Kind classifies why a Candidate is being offered, so the chooser UI can
// show a subtle hint (e.g. "project", "here", "global").
type Kind int

const (
	// KindProject is the nearest ancestor directory containing a .git or
	// .obsidian marker (a project or vault root).
	KindProject Kind = iota
	// KindHere is the launch directory itself (cwd).
	KindHere
	// KindGlobal is the user's home directory — the "absolute root" that,
	// once claimed, silently roots every future rootless launch under it.
	KindGlobal
	// KindMemory is not a disk location at all: choosing it opens docstate's
	// recovery store as :memory: instead of workDir/.rune/rune.db — no .rune
	// directory is ever created. Dir still carries cwd (the file-tree root
	// stays real), only Kind changes what the chooser and its caller do with
	// the candidate.
	KindMemory
)

// String returns the lowercase hint word for the candidate's kind.
func (k Kind) String() string {
	switch k {
	case KindProject:
		return "project"
	case KindHere:
		return "here"
	case KindGlobal:
		return "global"
	case KindMemory:
		return "memory"
	default:
		return ""
	}
}

// Candidate is one directory the user may pick as the workspace root.
type Candidate struct {
	Dir  string
	Kind Kind
}

// Prompt carries the deduped candidates to offer the user when no existing
// .rune/ was found anywhere on the walk up from cwd.
type Prompt struct {
	Candidates []Candidate
	// Default is the index into Candidates the chooser should preselect:
	// the project candidate if present, else the cwd candidate. Never the
	// home candidate.
	Default int
}

// Result is the outcome of Resolve. Exactly one of the two fields is
// meaningful (§1.7 — no sentinel abuse): a non-empty WorkDir means the root
// was decided silently; a non-nil Prompt means the caller must ask the user.
type Result struct {
	WorkDir string
	Prompt  *Prompt
}

// Resolve walks bottom-up from cwd looking for an existing rune workspace
// (.rune/) or, failing that, project/vault markers (.git/.obsidian) to offer
// as prompt candidates. Exactly one fsys.ReadDir call is made per directory
// level visited — a ReadDir error is treated as "no markers here" and the
// walk keeps climbing (fail-safe: discovery never halts the app).
//
// The walk climbs through and including home, then stops; if cwd is not
// under home it climbs all the way to the filesystem root instead. An empty
// home is handled gracefully: the ceiling becomes "/" and no home candidate
// is offered.
func Resolve(fsys vfs.FS, cwd, home string) Result {
	cwd = filepath.Clean(cwd)
	if home != "" {
		home = filepath.Clean(home)
	}

	ceiling := "/"
	if home != "" && isUnderOrEqual(cwd, home) {
		ceiling = home
	}

	var projectRoot string // nearest ancestor dir with a .git/.obsidian marker
	dir := cwd
	for {
		if entries, err := fsys.ReadDir(dir); err == nil {
			hasRune, hasProjectMarker := scan(entries)
			if hasRune {
				return Result{WorkDir: dir}
			}
			if hasProjectMarker && projectRoot == "" {
				projectRoot = dir
			}
		}

		if dir == ceiling {
			break
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			break // reached filesystem root without hitting ceiling (defensive)
		}
		dir = parent
	}

	return Result{Prompt: buildPrompt(cwd, home, projectRoot)}
}

// scan reports whether entries contains the .rune marker and whether it
// contains a .git or .obsidian marker, checked by name only (a marker counts
// whether it is a directory or a file).
func scan(entries []fs.DirEntry) (hasRune, hasProjectMarker bool) {
	for _, e := range entries {
		switch e.Name() {
		case markerRune:
			hasRune = true
		case markerGit, markerObsidian:
			hasProjectMarker = true
		}
	}
	return
}

// buildPrompt assembles the deduped candidate list in priority order:
// projectRoot (if any), cwd, home (if non-empty). Default is the project
// candidate's index if present, else cwd's.
func buildPrompt(cwd, home, projectRoot string) *Prompt {
	seen := make(map[string]bool, 3)
	var candidates []Candidate
	add := func(dir string, kind Kind) {
		if seen[dir] {
			return
		}
		seen[dir] = true
		candidates = append(candidates, Candidate{Dir: dir, Kind: kind})
	}

	defaultIdx := -1
	if projectRoot != "" {
		add(projectRoot, KindProject)
		defaultIdx = 0
	}
	add(cwd, KindHere)
	if home != "" {
		add(home, KindGlobal)
	}
	if defaultIdx == -1 {
		for i, c := range candidates {
			if c.Dir == cwd {
				defaultIdx = i
				break
			}
		}
	}

	// KindMemory is appended directly, bypassing add/seen: it never creates
	// anything at cwd, so it must never be treated as a duplicate of the
	// KindHere candidate that shares the same Dir — deduping it away would
	// silently remove the option from the menu.
	candidates = append(candidates, Candidate{Dir: cwd, Kind: KindMemory})

	return &Prompt{Candidates: candidates, Default: defaultIdx}
}

// isUnderOrEqual reports whether path is base or a descendant of base.
// Both arguments must already be filepath.Clean-ed absolute paths.
func isUnderOrEqual(path, base string) bool {
	if path == base {
		return true
	}
	prefix := base
	if !strings.HasSuffix(prefix, string(filepath.Separator)) {
		prefix += string(filepath.Separator)
	}
	return strings.HasPrefix(path, prefix)
}
