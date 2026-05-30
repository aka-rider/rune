package editor

import (
	"errors"
	"fmt"
	"strings"
)

// reservedChars are disallowed in Obsidian filenames on all platforms.
const reservedChars = `/\:*?"<>|`

// validateFilename returns a descriptive error for names that are:
// empty, whitespace-only, "." or "..", contain reserved characters,
// or exceed 255 bytes (macOS/Linux hard limit).
func validateFilename(name string) error {
	if strings.TrimSpace(name) == "" {
		return errors.New("filename cannot be empty")
	}
	if name == "." || name == ".." {
		return errors.New(`"." and ".." are not valid filenames`)
	}
	for _, r := range name {
		if strings.ContainsRune(reservedChars, r) {
			return fmt.Errorf("character %q is not allowed in filenames", r)
		}
	}
	if len(name) > 255 {
		return errors.New("filename exceeds 255 bytes")
	}
	return nil
}
