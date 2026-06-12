package markdownedit

import (
	"os"
	"path/filepath"
	"strings"
)

type linkResolutionMode int

const (
	resolveFromFileDir linkResolutionMode = iota
	resolveFromCWD
	resolveAbsolute
	resolveSkip
)

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
	stripped := raw
	if strings.HasPrefix(raw, "./") {
		stripped = raw[2:]
	}
	if !strings.ContainsRune(stripped, '/') && !strings.ContainsRune(stripped, '\\') {
		return resolveFromFileDir
	}
	return resolveFromCWD
}

// resolveLink resolves a raw link target to an absolute file path.
func resolveLink(raw string, fileDir string, appendMD bool, existCheck bool) string {
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

	target := strings.TrimSpace(raw)
	explicitRelative := strings.HasPrefix(target, "./")
	if explicitRelative {
		target = target[2:]
	}
	if appendMD && filepath.Ext(target) == "" {
		target = target + ".md"
	}

	cwd, _ := os.Getwd()

	switch mode {
	case resolveFromFileDir:
		if fileDir != "" {
			candidate := filepath.Join(fileDir, target)
			if existCheck && fileExistsForLink(candidate) {
				return filepath.Clean(candidate)
			}
			if !existCheck && fileDir != "" {
				return filepath.Clean(candidate)
			}
		}
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

// resolveEmbed resolves an image/embed target to an existing on-disk path.
// Uses m.dir as the base directory.
func (m Model) resolveEmbed(target string) string {
	if resolved := resolveLink(target, m.dir, false, true); resolved != "" {
		return resolved
	}
	if filepath.Ext(strings.TrimSpace(target)) == "" {
		if resolved := resolveLink(target, m.dir, true, true); resolved != "" {
			return resolved
		}
	}
	return ""
}

// resolveNavigation resolves a navigable link target.
func (m Model) resolveNavigation(target string, appendMD bool) string {
	return resolveLink(target, m.dir, appendMD, false)
}
