package opentabs

import "testing"

func TestTabHandle_Equal(t *testing.T) {
	cases := []struct {
		name string
		a, b TabHandle
		want bool
	}{
		// DocID != 0: authoritative identity; path is irrelevant (rename-safe).
		{"same doc, path changed", TabHandle{5, "old.md"}, TabHandle{5, "new.md"}, true},
		{"same doc, same path", TabHandle{5, "a.md"}, TabHandle{5, "a.md"}, true},
		{"different docs, same path", TabHandle{5, "a.md"}, TabHandle{6, "a.md"}, false},
		{"different docs, different paths", TabHandle{5, "a.md"}, TabHandle{6, "b.md"}, false},
		// DocID == 0: path is the only discriminator (virtual docs).
		{"virtual: same path", TabHandle{0, "/help"}, TabHandle{0, "/help"}, true},
		{"virtual: different paths", TabHandle{0, "/help"}, TabHandle{0, ""}, false},
		// Mixed: one side has real docID.
		{"zero vs nonzero docID", TabHandle{0, "a.md"}, TabHandle{1, "a.md"}, false},
		{"nonzero vs zero docID", TabHandle{1, "a.md"}, TabHandle{0, "a.md"}, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := c.a.Equal(c.b); got != c.want {
				t.Errorf("a.Equal(b) = %v, want %v", got, c.want)
			}
			if got := c.b.Equal(c.a); got != c.want {
				t.Errorf("b.Equal(a) = %v, want %v (symmetry violated)", got, c.want)
			}
		})
	}
}
