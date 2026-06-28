//go:build fuzzing

package footer

// PendingKey returns the current chord-pending key ("c", "d", or "").
// Used by the fuzz driver to populate Snapshot.ChordPending.
func (m Model) PendingKey() string { return m.pendingKey }

// GuardOptionCount returns the number of active guard options.
// Non-zero means the footer is showing a data-loss guard prompt.
func (m Model) GuardOptionCount() int { return len(m.guardOptions) }
