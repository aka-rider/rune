package editortest

import "testing"

type mockTB struct {
	testing.TB
	failed bool
}

func (m *mockTB) Errorf(format string, args ...any) {
	m.failed = true
}

func (m *mockTB) Fatalf(format string, args ...any) {
	m.failed = true
}

func (m *mockTB) Helper() {}
