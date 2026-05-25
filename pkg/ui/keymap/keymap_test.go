package keymap

import (
	"reflect"
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

// TestKeybindingCollisionsWithFieldNames provides detailed collision reporting
// showing which Bindings fields conflict.
func TestKeybindingCollisionsWithFieldNames(t *testing.T) {
	keys := Default()
	v := reflect.ValueOf(keys)
	typ := v.Type()

	// Map: physical key string → list of field names that use it
	keyToFields := make(map[string][]string)

	for i := 0; i < v.NumField(); i++ {
		field := v.Field(i)
		fieldName := typ.Field(i).Name

		// Each field is a key.Binding; call Keys() to get its physical keys
		keysMethod := field.MethodByName("Keys")
		if !keysMethod.IsValid() {
			continue
		}
		result := keysMethod.Call(nil)
		if len(result) == 0 {
			continue
		}
		keySlice, ok := result[0].Interface().([]string)
		if !ok {
			continue
		}
		for _, k := range keySlice {
			keyToFields[k] = append(keyToFields[k], fieldName)
		}
	}

	for keyStr, fields := range keyToFields {
		if len(fields) > 1 {
			t.Errorf("physical key %q appears in multiple bindings: %v", keyStr, fields)
		}
	}
}

// TestAllBindingsHaveKeys ensures no binding field is left empty.
func TestAllBindingsHaveKeys(t *testing.T) {
	keys := Default()
	v := reflect.ValueOf(keys)
	typ := v.Type()

	for i := 0; i < v.NumField(); i++ {
		field := v.Field(i)
		fieldName := typ.Field(i).Name

		keysMethod := field.MethodByName("Keys")
		if !keysMethod.IsValid() {
			continue
		}
		result := keysMethod.Call(nil)
		if len(result) == 0 {
			continue
		}
		keySlice, ok := result[0].Interface().([]string)
		if !ok {
			continue
		}
		if len(keySlice) == 0 {
			t.Errorf("binding %q has no physical keys assigned", fieldName)
		}
	}
}
