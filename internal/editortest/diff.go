package editortest

import (
	"bufio"
	"fmt"
	"strings"
)

// change represents a single line change in a unified diff.
type change struct {
	op    string // "-", "+", " "
	index int    // index in original (A)
	value string
}

// UnifiedDiff returns a unified diff of two byte slices, line by line,
// with context lines. It produces output suitable for test failure messages.
func UnifiedDiff(a, b []byte) string {
	linesA := splitLines(a)
	linesB := splitLines(b)

	var changes []change

	// Simple LCS-based diff with context
	lcs := computeLCS(linesA, linesB)

	// Build diff from LCS
	i, j := 0, 0
	for i < len(linesA) && j < len(linesB) {
		if linesA[i] == linesB[j] {
			changes = append(changes, change{" ", i, linesA[i]})
			i++
			j++
		} else {
			// Check if linesA[i] is in the LCS
			foundInA := false
			for _, idx := range lcs {
				if idx == i {
					foundInA = true
					break
				}
			}
			if !foundInA {
				// Skip lines from A
				for i < len(linesA) {
					foundInA = false
					for _, idx := range lcs {
						if idx == i {
							foundInA = true
							break
						}
					}
					if foundInA {
						break
					}
					changes = append(changes, change{"-", i, linesA[i]})
					i++
				}
			}
			// Check if linesB[j] is in the LCS
			foundInB := false
			for _, idx := range lcs {
				if idx == j {
					foundInB = true
					break
				}
			}
			if !foundInB {
				for j < len(linesB) {
					foundInB = false
					for _, idx := range lcs {
						if idx == j {
							foundInB = true
							break
						}
					}
					if foundInB {
						break
					}
					changes = append(changes, change{"+", j, linesB[j]})
					j++
				}
			}
		}
	}

	// Remaining lines in A
	for i < len(linesA) {
		changes = append(changes, change{"-", i, linesA[i]})
		i++
	}

	// Remaining lines in B
	for j < len(linesB) {
		changes = append(changes, change{"+", j, linesB[j]})
		j++
	}

	return formatDiff(linesA, linesB, changes)
}

func splitLines(data []byte) []string {
	var lines []string
	scanner := bufio.NewScanner(strings.NewReader(string(data)))
	for scanner.Scan() {
		lines = append(lines, scanner.Text())
	}
	return lines
}

func computeLCS(a, b []string) []int {
	// Return indices in a that match
	m, n := len(a), len(b)
	if m == 0 || n == 0 {
		return nil
	}

	// DP table
	dp := make([][]int, m+1)
	for i := range dp {
		dp[i] = make([]int, n+1)
	}

	for i := m - 1; i >= 0; i-- {
		for j := n - 1; j >= 0; j-- {
			if a[i] == b[j] {
				dp[i][j] = dp[i+1][j+1] + 1
			} else {
				dp[i][j] = max(dp[i+1][j], dp[i][j+1])
			}
		}
	}

	// Backtrack to find LCS indices in a
	var result []int
	i, j := 0, 0
	for i < m && j < n {
		if a[i] == b[j] {
			result = append(result, i)
			i++
			j++
		} else if dp[i+1][j] >= dp[i][j+1] {
			i++
		} else {
			j++
		}
	}

	return result
}

func formatDiff(linesA, linesB []string, changes []change) string {
	var buf strings.Builder
	buf.WriteString("--- expected\n")
	buf.WriteString("+++ actual\n")

	for _, c := range changes {
		switch c.op {
		case "-":
			buf.WriteString(fmt.Sprintf("- %s\n", c.value))
		case "+":
			buf.WriteString(fmt.Sprintf("+ %s\n", c.value))
		case " ":
			buf.WriteString(fmt.Sprintf("  %s\n", c.value))
		}
	}

	return buf.String()
}
