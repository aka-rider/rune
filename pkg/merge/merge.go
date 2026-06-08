// Package merge wraps libgit2's git_merge_file for in-memory 3-way text merge.
//
// Build requirements:
//
//	apt install libgit2-dev        # Debian/Ubuntu (ships 1.7.x)
//	brew install libgit2           # macOS (ships 1.9.x)
//
// Build flags (go build / go test):
//
//	CGO_ENABLED=1 go build -tags libgit2 ./...
//
// The libgit2 tag is not strictly required — the build will work without it —
// but it makes the dependency explicit and allows build-tag gating in callers.
//
// If pkg-config cannot find libgit2 (e.g. custom install path), override:
//
//	CGO_CFLAGS="-I/usr/local/include" CGO_LDFLAGS="-L/usr/local/lib -lgit2" go build ./...
package merge

/*
#cgo pkg-config: libgit2

#include <git2.h>
#include <stdlib.h>

// mergeFile is a thin shim so Go does not need to handle C struct
// initialization macros (GIT_MERGE_FILE_INPUT_INIT, etc.) directly.
//
// Returns 0 on success. On success, *out is populated and must be freed
// by the caller via git_merge_file_result_free(&out).
//
// favor:
//   0 = GIT_MERGE_FILE_FAVOR_NORMAL  (emit conflict markers)
//   1 = GIT_MERGE_FILE_FAVOR_OURS
//   2 = GIT_MERGE_FILE_FAVOR_THEIRS
//   3 = GIT_MERGE_FILE_FAVOR_UNION
//
// flags is a bitmask of git_merge_file_flag_t values.
static int mergeFile(
    git_merge_file_result *out,
    const char *ancestor_ptr, size_t ancestor_len,
    const char *ours_ptr,     size_t ours_len,
    const char *theirs_ptr,   size_t theirs_len,
    const char *ancestor_label,
    const char *ours_label,
    const char *theirs_label,
    int favor,
    unsigned int flags
) {
    git_merge_file_input ancestor = GIT_MERGE_FILE_INPUT_INIT;
    git_merge_file_input ours     = GIT_MERGE_FILE_INPUT_INIT;
    git_merge_file_input theirs   = GIT_MERGE_FILE_INPUT_INIT;

    ancestor.ptr  = ancestor_ptr;
    ancestor.size = ancestor_len;
    ours.ptr      = ours_ptr;
    ours.size     = ours_len;
    theirs.ptr    = theirs_ptr;
    theirs.size   = theirs_len;

    git_merge_file_options opts = GIT_MERGE_FILE_OPTIONS_INIT;
    opts.ancestor_label = ancestor_label;
    opts.our_label      = ours_label;
    opts.their_label    = theirs_label;
    opts.favor          = (git_merge_file_favor_t)favor;
    opts.flags          = (git_merge_file_flag_t)flags;

    return git_merge_file(out, &ancestor, &ours, &theirs, &opts);
}
*/
import "C"

import (
	"errors"
	"fmt"
	"sync"
	"unsafe"
)

// libgit2 requires a global init/shutdown pair. The library maintains an
// internal reference count, so nested calls are safe. We call Init once at
// package load time and never call Shutdown — process exit handles cleanup.
// If you need explicit shutdown (e.g. plugin unloading), call
// C.git_libgit2_shutdown() yourself after all merge operations are complete.
var initOnce sync.Once

func ensureInit() {
	initOnce.Do(func() {
		C.git_libgit2_init()
	})
}

// Favor controls how conflicts are resolved.
type Favor int

const (
	// FavorNormal emits standard <<<<<<< / ======= / >>>>>>> conflict markers.
	// automergeable will be false in Result when conflicts exist.
	FavorNormal Favor = 0

	// FavorOurs silently resolves all conflicts by taking our side.
	FavorOurs Favor = 1

	// FavorTheirs silently resolves all conflicts by taking their side.
	FavorTheirs Favor = 2

	// FavorUnion includes both sides of every conflict region without markers.
	// Suitable for append-oriented content; not recommended for markdown prose
	// as it produces duplicate headings on structural conflicts.
	FavorUnion Favor = 3
)

// Flag is a bitmask controlling merge and diff behaviour.
// Values match git_merge_file_flag_t exactly.
type Flag uint32

const (
	// FlagDefault uses the xdiff histogram algorithm with no whitespace
	// handling. This is the recommended baseline for markdown.
	FlagDefault Flag = 0

	// FlagStyleMerge emits standard <<<<<<< / ======= / >>>>>>> markers.
	// This is the default conflict style when FlagStyleDiff3 is not set.
	FlagStyleMerge Flag = 1 << 0

	// FlagStyleDiff3 adds a ||||||| ancestor block inside conflict regions,
	// giving the user three-way context in the output file.
	FlagStyleDiff3 Flag = 1 << 1

	// FlagSimplifyAlnum condenses non-alphanumeric regions to reduce noise.
	FlagSimplifyAlnum Flag = 1 << 2

	// FlagIgnoreWhitespace ignores all whitespace. Dangerous for markdown
	// where leading spaces denote indented code blocks and list nesting.
	FlagIgnoreWhitespace Flag = 1 << 3

	// FlagIgnoreWhitespaceChange ignores changes in the amount of whitespace.
	FlagIgnoreWhitespaceChange Flag = 1 << 4

	// FlagIgnoreWhitespaceEOL ignores trailing whitespace only.
	// Safe for markdown; trailing spaces are not syntactically meaningful
	// in most renderers.
	FlagIgnoreWhitespaceEOL Flag = 1 << 5

	// FlagDiffPatience uses the patience diff algorithm instead of histogram.
	// On documents with many unique anchor lines (e.g. headings), patience
	// and histogram produce identical output. Prefer FlagDefault (histogram).
	FlagDiffPatience Flag = 1 << 6

	// FlagDiffMinimal uses exhaustive Myers diff (smallest possible edit script).
	// Slower than histogram; rarely better for prose.
	FlagDiffMinimal Flag = 1 << 7

	// FlagStyleZdiff3 uses zealous diff3 style (condensed ancestor hunks).
	// Requires libgit2 >= 1.4.
	FlagStyleZdiff3 Flag = 1 << 8

	// FlagAcceptConflicts writes conflict markers into the result and still
	// reports automergeable = true. Useful for tooling that wants to post-
	// process conflict markers rather than treat them as failures.
	// Requires libgit2 >= 1.4.
	FlagAcceptConflicts Flag = 1 << 9
)

// Options configures a merge operation.
type Options struct {
	// Favor controls conflict resolution strategy. Default: FavorNormal.
	Favor Favor

	// Flags is a bitmask of Flag values. Default: FlagDefault (histogram, no
	// whitespace handling). Recommended for markdown: FlagIgnoreWhitespaceEOL.
	Flags Flag

	// Labels for conflict markers. Empty strings use libgit2 defaults
	// ("ancestor", "ours", "theirs").
	AncestorLabel string
	OursLabel     string
	TheirsLabel   string
}

// DefaultOptions returns the recommended options for markdown file merging:
// histogram diff (xdiff default), trailing-whitespace ignored, normal conflict
// markers with labels. Callers should set OursLabel / TheirsLabel to
// meaningful branch or author names.
func DefaultOptions() Options {
	return Options{
		Favor:         FavorNormal,
		Flags:         FlagIgnoreWhitespaceEOL,
		AncestorLabel: "ancestor",
		OursLabel:     "ours",
		TheirsLabel:   "theirs",
	}
}

// Result is the output of a merge operation.
type Result struct {
	// Output is the merged content. When Conflicted is true, it contains
	// conflict markers (unless FavorUnion / FavorOurs / FavorTheirs was used,
	// in which case conflicts are silently resolved and Conflicted is false).
	Output []byte

	// Conflicted is true when the merge could not be completed automatically
	// and conflict markers were emitted into Output.
	// Always false when Favor != FavorNormal.
	Conflicted bool
}

// Merge performs an in-memory 3-way merge of ancestor, ours, and theirs using
// libgit2's git_merge_file (xdiff histogram algorithm).
//
// None of the inputs are modified. The function is safe to call concurrently:
// libgit2's merge-file path is stateless and does not reference a repository.
//
// Memory: the C-side result buffer is copied into Result.Output and then freed
// before Merge returns. No C memory escapes.
func Merge(ancestor, ours, theirs []byte, opts Options) (Result, error) {
	ensureInit()

	// C strings for labels — must stay alive across the C call.
	// cLabel returns nil for empty strings so libgit2 uses its defaults.
	ancestorLabel := cLabel(opts.AncestorLabel)
	oursLabel := cLabel(opts.OursLabel)
	theirsLabel := cLabel(opts.TheirsLabel)

	defer func() {
		if ancestorLabel != nil {
			C.free(unsafe.Pointer(ancestorLabel))
		}
		if oursLabel != nil {
			C.free(unsafe.Pointer(oursLabel))
		}
		if theirsLabel != nil {
			C.free(unsafe.Pointer(theirsLabel))
		}
	}()

	// Empty slices: pass a non-nil pointer to a zero byte so size == 0 is
	// valid. libgit2 checks size, not nullness, for empty content.
	ancestorPtr, ancestorLen := bytesForC(ancestor)
	oursPtr, oursLen := bytesForC(ours)
	theirsPtr, theirsLen := bytesForC(theirs)

	var out C.git_merge_file_result
	// Zero the result struct. libgit2 does not guarantee zero-init on error.
	out = C.git_merge_file_result{}

	rc := C.mergeFile(
		&out,
		ancestorPtr, ancestorLen,
		oursPtr, oursLen,
		theirsPtr, theirsLen,
		ancestorLabel,
		oursLabel,
		theirsLabel,
		C.int(opts.Favor),
		C.uint(opts.Flags),
	)

	// Always free the result, even on error, because libgit2 may have
	// partially populated it before failing.
	defer C.git_merge_file_result_free(&out)

	if rc < 0 {
		return Result{}, fmt.Errorf("merge: libgit2 error code %d: %w", int(rc), libgit2Error())
	}

	// Copy result bytes into Go memory before freeing.
	var output []byte
	if out.len > 0 && out.ptr != nil {
		output = C.GoBytes(unsafe.Pointer(out.ptr), C.int(out.len))
	}

	return Result{
		Output:     output,
		Conflicted: out.automergeable == 0,
	}, nil
}

// --- helpers -----------------------------------------------------------------

// cLabel converts a Go string to a C string allocated with C.CString.
// Returns nil if s is empty so libgit2 uses its built-in default label.
// Caller must C.free the returned pointer.
func cLabel(s string) *C.char {
	if s == "" {
		return nil
	}
	return C.CString(s)
}

// bytesForC returns a C pointer and size suitable for passing to mergeFile.
// For empty slices it returns a pointer to a static zero byte (never nil)
// with size 0, which is safe for libgit2's size-checked code paths.
func bytesForC(b []byte) (*C.char, C.size_t) {
	if len(b) == 0 {
		// Use a static sentinel so the pointer is non-nil but size is 0.
		var sentinel C.char
		return &sentinel, 0
	}
	return (*C.char)(unsafe.Pointer(&b[0])), C.size_t(len(b))
}

// libgit2Error retrieves the last libgit2 error message for the current thread.
// Returns a generic error if no message is available.
func libgit2Error() error {
	gerr := C.git_error_last()
	if gerr == nil {
		return errors.New("unknown libgit2 error")
	}
	return errors.New(C.GoString(gerr.message))
}
