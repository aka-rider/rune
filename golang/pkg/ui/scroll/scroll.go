package scroll

// Follow returns a new viewport offset so cursor stays within [offset, offset+size),
// keeping ≥ margin cells from each edge. When the cursor breaches a margin, the offset
// moves so the cursor lands jump cells inside the margin (jump=0 → minimal line-by-line
// scroll). The result is clamped to [0, max(0, total-size)], so margins are not enforced
// past the content ends (matches vim scrolloff).
func Follow(cursor, offset, size, total, margin, jump int) int {
	if size <= 0 {
		return 0
	}
	if margin*2 > size {
		margin = (size - 1) / 2
	}
	if margin < 0 {
		margin = 0
	}
	if jump > size-1-2*margin {
		jump = size - 1 - 2*margin
	}
	if jump < 0 {
		jump = 0
	}

	switch {
	case cursor < offset+margin:
		offset = cursor - margin - jump
	case cursor >= offset+size-1-margin:
		offset = cursor - (size - 1 - margin) + jump
	}

	maxOff := total - size
	if maxOff < 0 {
		maxOff = 0
	}
	if offset < 0 {
		offset = 0
	}
	if offset > maxOff {
		offset = maxOff
	}
	return offset
}
