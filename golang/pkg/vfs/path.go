package vfs

import (
	"log"
	"os"
	"strings"
	"sync"
)

var (
	homeOnce sync.Once
	homeDir  string
	homeErr  error
)

// NormPath replaces a leading $HOME prefix with ~.
// The home directory is resolved once and cached; a resolution failure
// is logged as a warning and the path is returned unchanged.
func NormPath(path string) string {
	homeOnce.Do(func() {
		homeDir, homeErr = os.UserHomeDir()
		if homeErr != nil {
			log.Printf("warn: os.UserHomeDir: %v", homeErr)
		}
	})
	if homeErr != nil {
		return path
	}
	rest, ok := strings.CutPrefix(path, homeDir)
	if !ok {
		return path
	}
	if rest == "" {
		return "~"
	}
	if rest[0] == '/' {
		return "~" + rest
	}
	return path
}
