package search

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// Span is a match range [Start, End) as byte offsets into the searched string.
type Span struct{ Start, End int }

// Find returns non-overlapping matches of query in content, left to right.
// Empty query or empty content returns nil.
// When caseInsensitive is true, both sides are compared via unicode.ToLower.
// Byte offsets refer to the original content string (not a lowercased copy).
func Find(content, query string, caseInsensitive bool) []Span {
	if query == "" || content == "" {
		return nil
	}

	// Pre-compute the query as a rune slice, optionally lowercased.
	var qRunes []rune
	if caseInsensitive {
		for _, r := range query {
			qRunes = append(qRunes, unicode.ToLower(r))
		}
	} else {
		for _, r := range query {
			qRunes = append(qRunes, r)
		}
	}
	qLen := len(qRunes)

	var results []Span
	i := 0 // byte position in content
	n := len(content)

	for i < n {
		// Try to match at byte position i.
		j := 0 // index into qRunes
		k := i // byte cursor during match attempt
		for j < qLen && k < n {
			r, size := utf8.DecodeRuneInString(content[k:])
			if size == 0 {
				size = 1
				r = utf8.RuneError
			}
			var cr rune
			if caseInsensitive {
				cr = unicode.ToLower(r)
			} else {
				cr = r
			}
			if cr != qRunes[j] {
				break
			}
			j++
			k += size
		}
		if j == qLen {
			// Full match at [i, k).
			results = append(results, Span{Start: i, End: k})
			// Non-overlapping: advance past the end of this match.
			i = k
		} else {
			// No match; advance by one rune.
			_, size := utf8.DecodeRuneInString(content[i:])
			if size == 0 {
				size = 1
			}
			i += size
		}
	}
	return results
}

// FuzzyMatch reports whether query is a case-insensitive subsequence of candidate.
// An empty query matches everything.
func FuzzyMatch(query, candidate string) bool {
	if query == "" {
		return true
	}
	qLow := strings.ToLower(query)
	cLow := strings.ToLower(candidate)

	qi := 0
	qRunes := []rune(qLow)
	qLen := len(qRunes)
	for _, r := range cLow {
		if qi < qLen && r == qRunes[qi] {
			qi++
		}
	}
	return qi >= qLen
}
