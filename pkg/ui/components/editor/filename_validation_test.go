package editor

import (
	"strings"
	"testing"
)

func TestValidateFilename_Valid(t *testing.T) {
	if err := validateFilename("my-note"); err != nil {
		t.Errorf("expected nil, got %v", err)
	}
}

func TestValidateFilename_ValidUnicode(t *testing.T) {
	for _, name := range []string{"café", "日本語"} {
		if err := validateFilename(name); err != nil {
			t.Errorf("expected nil for %q, got %v", name, err)
		}
	}
}

func TestValidateFilename_ValidLeadingDot(t *testing.T) {
	if err := validateFilename(".hidden"); err != nil {
		t.Errorf("expected nil, got %v", err)
	}
}

func TestValidateFilename_Empty(t *testing.T) {
	if err := validateFilename(""); err == nil {
		t.Error("expected error for empty string")
	}
}

func TestValidateFilename_WhitespaceOnly(t *testing.T) {
	if err := validateFilename("   "); err == nil {
		t.Error("expected error for whitespace-only string")
	}
}

func TestValidateFilename_DotOnly(t *testing.T) {
	for _, name := range []string{".", ".."} {
		if err := validateFilename(name); err == nil {
			t.Errorf("expected error for %q", name)
		}
	}
}

func TestValidateFilename_ReservedChars(t *testing.T) {
	for _, ch := range reservedChars {
		name := "note" + string(ch)
		if err := validateFilename(name); err == nil {
			t.Errorf("expected error for filename containing %q", ch)
		}
	}
}

func TestValidateFilename_TooLong(t *testing.T) {
	name := strings.Repeat("a", 256)
	if err := validateFilename(name); err == nil {
		t.Error("expected error for filename exceeding 255 bytes")
	}
}
