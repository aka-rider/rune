package editor

import (
	"os"
	"path/filepath"
	"strings"
)

// linkResolutionMode determines how a link path should be resolved.
type linkResolutionMode int

const (
	resolveFromFileDir linkResolutionMode = iota // basename or ./ prefix
	resolveFromCWD                                // path with directory component
	resolveAbsolute                               // absolute path
	resolveSkip                                   // remote URL, skip
)

// classifyLink determines the resolution mode for a raw link target.
func classifyLink(raw string) linkResolutionMode {
	if raw == "" {
		return resolveSkip
	}
	if filepath.IsAbs(raw) {
		return resolveAbsolute
	}
	lower := strings.ToLower(raw)
	if strings.HasPrefix(lower, "http://") || strings.HasPrefix(lower, "https://") || strings.HasPrefix(lower, "data:") {
		return resolveSkip
	}
	// Strip leading ./ to check if what remains is a basename
	stripped := raw
	if strings.HasPrefix(raw, "./") {
		stripped = raw[2:]
	}
	// If the stripped path has no directory separator, it's file-relative
	if !strings.ContainsRune(stripped, '/') && !strings.ContainsRune(stripped, '\\') {
		return resolveFromFileDir
	}
	return resolveFromCWD
}

// resolveLink resolves a raw link target to an absolute file path.
//
// Parameters:
//   - raw: the raw link target from the markdown source
//   - filePath: the currently open file's absolute path (for file's directory)
//   - appendMD: if true and the target has no extension, append .md (Obsidian-like)
//   - existCheck: if true, only return the path if the file actually exists on disk
//
// Resolution order:
//   - basename or ./ links → file's directory → CWD (unless ./, then no fallback)
//   - path links → CWD
//   - absolute → as-is
//   - URLs → "" (skipped)
func resolveLink(raw string, filePath string, appendMD bool, existCheck bool) string {
	mode := classifyLink(raw)
	switch mode {
	case resolveSkip:
		return ""
	case resolveAbsolute:
		cleaned := filepath.Clean(raw)
		if existCheck && !fileExistsForLink(cleaned) {
			return ""
		}
		return cleaned
	}

	// Determine the target path (with optional .md suffix)
	target := strings.TrimSpace(raw)

	// Strip ./ prefix for resolution (it means "file's dir", not a real component)
	explicitRelative := strings.HasPrefix(target, "./")
	if explicitRelative {
		target = target[2:]
	}

	// Append .md if no extension (Obsidian-like for navigation links)
	if appendMD && filepath.Ext(target) == "" {
		target = target + ".md"
	}

	fileDir := ""
	if filePath != "" {
		fileDir = filepath.Dir(filePath)
	}
	cwd, _ := os.Getwd()

	switch mode {
	case resolveFromFileDir:
		// Try file's directory first
		if fileDir != "" {
			candidate := filepath.Join(fileDir, target)
			if existCheck && fileExistsForLink(candidate) {
				return filepath.Clean(candidate)
			}
			if !existCheck && fileDir != "" {
				return filepath.Clean(candidate)
			}
		}
		// CWD fallback (only for basename, NOT for explicit ./)
		if !explicitRelative && cwd != "" {
			candidate := filepath.Join(cwd, target)
			if existCheck && fileExistsForLink(candidate) {
				return filepath.Clean(candidate)
			}
			if !existCheck {
				return filepath.Clean(candidate)
			}
		}
		return ""
	case resolveFromCWD:
		candidate := filepath.Join(cwd, target)
		if existCheck && !fileExistsForLink(candidate) {
			return ""
		}
		return filepath.Clean(candidate)
	default:
		return ""
	}
}

func fileExistsForLink(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}
