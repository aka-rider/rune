//go:build !darwin

package microphone

import (
	"context"
	"errors"
)

// Start is not supported on non-darwin platforms.
func Start(_ context.Context) (<-chan []byte, error) {
	return nil, errors.New("microphone capture not supported on this platform")
}
