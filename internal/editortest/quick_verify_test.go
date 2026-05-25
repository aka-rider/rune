package editortest

import "testing"

func TestQuickVerify(t *testing.T) {
	// Test forward selection round-trip
	s1 := "[hello]world"
	ts1, err1 := ParseState(s1)
	if err1 != nil {
		t.Fatalf("ParseState(%q): unexpected error: %v", s1, err1)
	}
	formatted1 := FormatState(ts1)
	if formatted1 != s1 {
		t.Errorf("Forward selection round-trip: %q -> %q (expected %q)", s1, formatted1, s1)
	}

	// Test backward selection round-trip
	s2 := "]hello[world"
	ts2, err2 := ParseState(s2)
	if err2 != nil {
		t.Fatalf("ParseState(%q): unexpected error: %v", s2, err2)
	}
	formatted2 := FormatState(ts2)
	if formatted2 != s2 {
		t.Errorf("Backward selection round-trip: %q -> %q (expected %q)", s2, formatted2, s2)
	}

	// Test simple cursor round-trip
	s3 := "hello|world"
	ts3, err3 := ParseState(s3)
	if err3 != nil {
		t.Fatalf("ParseState(%q): unexpected error: %v", s3, err3)
	}
	formatted3 := FormatState(ts3)
	if formatted3 != s3 {
		t.Errorf("Simple cursor round-trip: %q -> %q (expected %q)", s3, formatted3, s3)
	}

	// Test cursor at start
	s4 := "|hello"
	ts4, err4 := ParseState(s4)
	if err4 != nil {
		t.Fatalf("ParseState(%q): unexpected error: %v", s4, err4)
	}
	formatted4 := FormatState(ts4)
	if formatted4 != s4 {
		t.Errorf("Cursor at start round-trip: %q -> %q (expected %q)", s4, formatted4, s4)
	}

	// Test cursor at end
	s5 := "hello|"
	ts5, err5 := ParseState(s5)
	if err5 != nil {
		t.Fatalf("ParseState(%q): unexpected error: %v", s5, err5)
	}
	formatted5 := FormatState(ts5)
	if formatted5 != s5 {
		t.Errorf("Cursor at end round-trip: %q -> %q (expected %q)", s5, formatted5, s5)
	}

	// Test unclosed bracket returns error
	_, err6 := ParseState("hello[world")
	if err6 == nil {
		t.Error("ParseState unclosed '[': expected error, got nil")
	}

	// Test orphan ] returns error
	_, err7 := ParseState("hello]world")
	if err7 == nil {
		t.Error("ParseState orphan ']': expected error, got nil")
	}

	// Test no cursor marker returns error
	_, err8 := ParseState("hello")
	if err8 == nil {
		t.Error("ParseState no cursor: expected error, got nil")
	}

	// Test escape sequences round-trip
	s9 := "a\\|b"
	ts9, err9 := ParseState(s9)
	if err9 != nil {
		t.Fatalf("ParseState(%q): unexpected error: %v", s9, err9)
	}
	formatted9 := FormatState(ts9)
	if formatted9 != s9 {
		t.Errorf("Escape round-trip: %q -> %q (expected %q)", s9, formatted9, s9)
	}

	// Test empty string returns error
	_, err10 := ParseState("")
	if err10 == nil {
		t.Error("ParseState empty string: expected error, got nil")
	}
}
