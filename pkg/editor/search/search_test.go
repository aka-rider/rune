package search_test

import (
	"testing"

	"rune/pkg/editor/search"
)

func TestFind(t *testing.T) {
	tests := []struct {
		name            string
		content         string
		query           string
		caseInsensitive bool
		want            []search.Span
	}{
		// Basic cases
		{name: "empty query", content: "hello", query: "", want: nil},
		{name: "empty content", content: "", query: "hi", want: nil},
		{name: "no match", content: "hello world", query: "xyz", want: nil},

		// Single match
		{name: "single ASCII", content: "hello world", query: "world", want: []search.Span{{6, 11}}},
		{name: "match at start", content: "hello world", query: "hello", want: []search.Span{{0, 5}}},
		{name: "full string match", content: "abc", query: "abc", want: []search.Span{{0, 3}}},

		// Multiple non-overlapping matches
		{
			name:    "multiple matches",
			content: "abcabc",
			query:   "abc",
			want:    []search.Span{{0, 3}, {3, 6}},
		},

		// Multiline
		{
			name:    "multiline match",
			content: "foo\nbar\nbaz",
			query:   "bar",
			want:    []search.Span{{4, 7}},
		},

		// Case-insensitive
		{
			name:            "case insensitive",
			content:         "Hello World",
			query:           "hello",
			caseInsensitive: true,
			want:            []search.Span{{0, 5}},
		},
		{
			name:            "case insensitive mixed",
			content:         "HELLO hello Hello",
			query:           "hello",
			caseInsensitive: true,
			want:            []search.Span{{0, 5}, {6, 11}, {12, 17}},
		},

		// Case-sensitive: no match
		{
			name:            "case sensitive no match",
			content:         "HELLO",
			query:           "hello",
			caseInsensitive: false,
			want:            nil,
		},

		// CJK / multibyte
		{
			name:    "CJK match",
			content: "你好世界",
			query:   "世界",
			// "你" = 3 bytes, "好" = 3 bytes, "世" starts at 6, "界" at 9; end at 12
			want: []search.Span{{6, 12}},
		},
		{
			name:    "CJK no offset drift",
			content: "abc你好def",
			query:   "def",
			// "abc" = 3 bytes, "你好" = 6 bytes → "def" starts at 9
			want: []search.Span{{9, 12}},
		},

		// Emoji
		{
			name:    "emoji match",
			content: "hi 🎉 world",
			query:   "world",
			// "hi " = 3, "🎉" = 4 bytes, " " = 1 → world starts at 8
			want: []search.Span{{8, 13}},
		},

		// Unicode case folding
		{
			name:            "unicode fold sharp-s",
			content:         "Straße",
			query:           "straße",
			caseInsensitive: true,
			// S→s, t→t, r→r, a→a, ß→ß, e→e (ß stays ß under ToLower)
			want: []search.Span{{0, 7}}, // "Straße" is S(1)+t(1)+r(1)+a(1)+ß(2)+e(1)=7 bytes
		},

		// Non-overlapping guarantee
		{
			name:    "non-overlapping",
			content: "aaaa",
			query:   "aa",
			want:    []search.Span{{0, 2}, {2, 4}},
		},

		// Query longer than content
		{
			name:    "query longer than content",
			content: "hi",
			query:   "hello",
			want:    nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := search.Find(tt.content, tt.query, tt.caseInsensitive)
			if len(got) != len(tt.want) {
				t.Fatalf("Find(%q, %q, %v) = %v, want %v", tt.content, tt.query, tt.caseInsensitive, got, tt.want)
			}
			for i := range got {
				if got[i] != tt.want[i] {
					t.Errorf("span[%d] = %v, want %v", i, got[i], tt.want[i])
				}
			}
		})
	}
}

func TestFuzzyMatch(t *testing.T) {
	tests := []struct {
		name      string
		query     string
		candidate string
		want      bool
	}{
		{"empty query", "", "anything", true},
		{"empty both", "", "", true},
		{"exact match", "foo", "foo", true},
		{"subsequence", "fc", "foo car", true},
		{"not subsequence", "xyz", "hello", false},
		{"case insensitive", "FC", "foo car", true},
		{"prefix match", "hel", "hello world", true},
		{"scattered", "aeo", "abcdefgho", true},
		{"out of order", "ba", "abc", false},
		{"single char match", "f", "foo", true},
		{"single char no match", "z", "foo", false},
		{"candidate shorter than query", "hello world", "hi", false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := search.FuzzyMatch(tt.query, tt.candidate)
			if got != tt.want {
				t.Errorf("FuzzyMatch(%q, %q) = %v, want %v", tt.query, tt.candidate, got, tt.want)
			}
		})
	}
}
