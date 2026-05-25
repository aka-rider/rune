package keymap

import (
        "testing"
)

func TestNoKeybindingCollisions(t *testing.T) {
        keys := Default()
        physicalKeys := keys.AllPhysicalKeys()
        
        seen := make(map[string]bool)
        for _, k := range physicalKeys {
                if seen[k] {
                        t.Errorf("duplicate physical key string detected: %q", k)
                }
                seen[k] = true
        }

        err := keys.ValidateNoPhysicalKeyCollisions()
        if err != nil {
                t.Errorf("ValidateNoPhysicalKeyCollisions returned error: %v", err)
        }
}
