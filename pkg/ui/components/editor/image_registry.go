package editor

import (
	"fmt"
	"time"

	"rune/pkg/imagekit"
)

// imageState tracks an image entry through its decode/transmit lifecycle.
type imageState int

const (
	pendingDecode imageState = iota
	pendingTransmit
	live
	failed
)

// imageEntry holds metadata for one document image. Decoded pixels are NEVER
// stored here — they live only inside the I/O Cmd closures — so value-receiver
// Model copies stay cheap.
type imageEntry struct {
	path    string // raw markdown destination (registry key)
	absPath string // resolved absolute path on disk
	id      uint32 // Kitty image ID (24-bit)
	mtime   int64  // source file modtime when last decoded
	cols    int
	rows    int
	pxW     int
	pxH     int
	state   imageState
	altText string

	// wasExpanded tracks whether this image was expanded (multi-row) in the
	// previous syncDisplay cycle. Used to detect collapse transitions and
	// trigger a full screen repaint to clear ghost pixels.
	wasExpanded bool

	// iTerm2 inline rendering: pre-encoded OSC 1337 row-slices stored after
	// encode so View-time placement is a cheap string build (no re-encode).
	// Each element is one terminal row's worth of image pixels, independently
	// placeable for viewport clipping.
	iterm2Slices []string

	// Animation fields for animated GIFs. Each frame is transmitted as its own
	// Kitty image; frameIDs[frameIdx] is the image the placeholder cells
	// currently reference (via fg color), so advancing a frame only changes the
	// cell foreground — geometry never reflows.
	animated   bool
	frameIDs   []uint32
	frameIdx   int
	frameCount int
	delays     []time.Duration
	loopCount  int // 0 = forever, <0 = play once, >0 = explicit count
	loopsDone  int
	tickGen    int  // generation guard: stale ticks (older gen) are dropped
	ticking    bool // whether a tick chain is currently scheduled
}

// imageRegistry maps document images by raw path and by Kitty ID. All mutators
// are copy-on-write value methods: each clones the maps so value-receiver Model
// copies never alias a shared map.
type imageRegistry struct {
	byPath map[string]imageEntry
	byID   map[uint32]imageEntry
}

func newImageRegistry() imageRegistry {
	return imageRegistry{
		byPath: map[string]imageEntry{},
		byID:   map[uint32]imageEntry{},
	}
}

func (r imageRegistry) clone() imageRegistry {
	nb := make(map[string]imageEntry, len(r.byPath))
	for k, v := range r.byPath {
		nb[k] = v
	}
	ni := make(map[uint32]imageEntry, len(r.byID))
	for k, v := range r.byID {
		ni[k] = v
	}
	return imageRegistry{byPath: nb, byID: ni}
}

// get returns the entry for a raw path and whether it exists.
func (r imageRegistry) get(path string) (imageEntry, bool) {
	e, ok := r.byPath[path]
	return e, ok
}

// upsert inserts or replaces an entry (keyed by both path and ID) and returns
// the new registry.
func (r imageRegistry) upsert(e imageEntry) imageRegistry {
	nr := r.clone()
	// Drop any stale ID mappings for this path (base + frames).
	if old, ok := nr.byPath[e.path]; ok {
		delete(nr.byID, old.id)
		for _, fid := range old.frameIDs {
			delete(nr.byID, fid)
		}
	}
	nr.byPath[e.path] = e
	if e.id != 0 {
		nr.byID[e.id] = e
	}
	for _, fid := range e.frameIDs {
		if fid != 0 {
			nr.byID[fid] = e
		}
	}
	return nr
}

// remove deletes the entry for a raw path and returns the new registry.
func (r imageRegistry) remove(path string) imageRegistry {
	nr := r.clone()
	if e, ok := nr.byPath[path]; ok {
		delete(nr.byID, e.id)
		for _, fid := range e.frameIDs {
			delete(nr.byID, fid)
		}
		delete(nr.byPath, path)
	}
	return nr
}

// allocFrameIDs returns n distinct, collision-free 24-bit IDs for the frames of
// an animated image at absPath.
func (r imageRegistry) allocFrameIDs(absPath string, n int) []uint32 {
	ids := make([]uint32, 0, n)
	used := map[uint32]bool{}
	for i := 0; i < n; i++ {
		seed := fmt.Sprintf("%s#frame%d", absPath, i)
		id := imagekit.AllocID(seed)
		for {
			existing, taken := r.byID[id]
			if !used[id] && (!taken || existing.absPath == absPath) {
				break
			}
			id++
			if id == 0 || id > idMask24Editor {
				id = 1
			}
		}
		used[id] = true
		ids = append(ids, id)
	}
	return ids
}

// allocFreeID returns a deterministic 24-bit ID for absPath, linear-probing on
// collision with a different path so two distinct images never share an ID.
func (r imageRegistry) allocFreeID(absPath string) uint32 {
	id := imagekit.AllocID(absPath)
	for i := 0; i < idMask24Editor; i++ {
		existing, taken := r.byID[id]
		if !taken || existing.absPath == absPath {
			return id
		}
		id++
		if id == 0 || id > idMask24Editor {
			id = 1
		}
	}
	return id
}

const idMask24Editor = 0xFFFFFF

// IDFor returns the Kitty ID assigned to a raw path, or 0 if unknown.
func (r imageRegistry) IDFor(path string) uint32 {
	if e, ok := r.byPath[path]; ok {
		return e.id
	}
	return 0
}

// liveIDs returns the IDs of all images currently transmitted (live) on the
// terminal, including every animation frame ID.
func (r imageRegistry) liveIDs() []uint32 {
	seen := map[uint32]bool{}
	var ids []uint32
	add := func(id uint32) {
		if id != 0 && !seen[id] {
			seen[id] = true
			ids = append(ids, id)
		}
	}
	for _, e := range r.byPath {
		if e.state != live {
			continue
		}
		add(e.id)
		for _, fid := range e.frameIDs {
			add(fid)
		}
	}
	return ids
}
