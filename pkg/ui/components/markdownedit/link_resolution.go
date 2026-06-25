package markdownedit

// Relative-reference resolution — ONE resolver (resolveRef), reused by both link
// following and image embeds:
//
//	follow (mouse/Enter) ─┐
//	image embed ──────────┴─→ resolveRef(target, docDir, root) → abs path (existence-checked)
//
// The base directories are NEVER stored/derived state: they come from the golden
// source — the open document's path (docDir = filepath.Dir(docPath)) — and the
// static workspace root (launch CWD). A relative target resolves against the
// document's own folder first, then the root. External schemes (http/https/mailto)
// bypass this and go to the OS opener (isExternalURL is that allowlist).
//
// Ctrl is an alias, not a mode: Ctrl+Enter ≡ Enter and Ctrl+double-click ≡
// double-click — a plain follow, no "new tab".

import (
	"os"
	"path/filepath"
	"strings"
)

// resolveRef resolves a relative reference against docDir first, then root,
// returning the absolute path of the first that exists. appendMD adds ".md" to an
// extensionless target (wiki/markdown links). An absolute target is returned iff it
// exists. This is the single resolver shared by link-follow and image embeds.
func resolveRef(target, docDir, root string, appendMD bool) (string, bool) {
	target = strings.TrimSpace(target)
	if target == "" {
		return "", false
	}
	target = strings.TrimPrefix(target, "./")
	if appendMD && filepath.Ext(target) == "" {
		target += ".md"
	}
	if filepath.IsAbs(target) {
		clean := filepath.Clean(target)
		return clean, fileExistsForLink(clean)
	}
	for _, base := range [2]string{docDir, root} {
		if base == "" {
			continue
		}
		cand := filepath.Clean(filepath.Join(base, target))
		if fileExistsForLink(cand) {
			return cand, true
		}
	}
	return "", false
}

func fileExistsForLink(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

// resolveEmbed resolves an image/embed target to an existing on-disk path, against
// the open document's folder then the workspace root — the SAME bases (and the same
// resolver) as link following.
func (m Model) resolveEmbed(target string) string {
	if abs, ok := resolveRef(target, m.docDir(), m.root, false); ok {
		return abs
	}
	if filepath.Ext(strings.TrimSpace(target)) == "" {
		if abs, ok := resolveRef(target, m.docDir(), m.root, true); ok {
			return abs
		}
	}
	return ""
}

// isExternalURL reports whether raw is a scheme that should open in the OS
// default handler rather than inside rune. This allowlist is the security
// boundary for the shell-out in workspace_nav.go: only these schemes ever reach
// the OS opener, so a link can never invoke an arbitrary handler (e.g. file://,
// javascript:). data: is intentionally excluded — it is non-navigable.
func isExternalURL(raw string) bool {
	lower := strings.ToLower(strings.TrimSpace(raw))
	return strings.HasPrefix(lower, "http://") ||
		strings.HasPrefix(lower, "https://") ||
		strings.HasPrefix(lower, "mailto:")
}
