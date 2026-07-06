package docstate

import (
	"io/fs"

	"rune/pkg/vfs"
)

// hookFS wraps a vfs.FS and lets tests inject side effects at specific
// method-call points — the "fault-hook FS wrapper" WP4's validation calls
// for, used to simulate a writer racing Materialize/Probe from a concurrent
// process. Each hook receives the WRAPPED (real) FS, not the hookFS itself,
// so a hook's own disk operations never re-trigger the hook.
type hookFS struct {
	vfs.FS
	beforeExchange   func(real vfs.FS)
	beforeRenameExcl func(real vfs.FS)
	beforeWriteFile  func(real vfs.FS)
}

func (h *hookFS) Exchange(a, b string) error {
	if h.beforeExchange != nil {
		h.beforeExchange(h.FS)
	}
	return h.FS.Exchange(a, b)
}

func (h *hookFS) RenameExcl(oldPath, newPath string) error {
	if h.beforeRenameExcl != nil {
		h.beforeRenameExcl(h.FS)
	}
	return h.FS.RenameExcl(oldPath, newPath)
}

func (h *hookFS) WriteFile(name string, data []byte, perm fs.FileMode) error {
	if h.beforeWriteFile != nil {
		h.beforeWriteFile(h.FS)
	}
	return h.FS.WriteFile(name, data, perm)
}

var _ vfs.FS = (*hookFS)(nil)

// unsupportedExchangeFS forces every Exchange call to fail with
// vfs.ErrUnsupported, exercising Materialize's step-7 probe+rename fallback
// (Mem's Exchange is otherwise always supported).
type unsupportedExchangeFS struct {
	vfs.FS
}

func (u unsupportedExchangeFS) Exchange(a, b string) error {
	return vfs.ErrUnsupported
}

var _ vfs.FS = unsupportedExchangeFS{}
