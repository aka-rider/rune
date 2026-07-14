package markdownedit

import (
	"fmt"

	"rune/pkg/imagekit"
)

type imageIDAllocator struct {
	byID map[uint32]string
}

func newImageIDAllocator() imageIDAllocator {
	return imageIDAllocator{byID: map[uint32]string{}}
}

func (a imageIDAllocator) clone() imageIDAllocator {
	ni := make(map[uint32]string, len(a.byID))
	for k, v := range a.byID {
		ni[k] = v
	}
	return imageIDAllocator{byID: ni}
}

// AllocFreeID returns a deterministic 24-bit ID for absPath, probing on collision.
func (a imageIDAllocator) AllocFreeID(absPath string) (uint32, imageIDAllocator) {
	id := imagekit.AllocID(absPath)
	na := a.clone()
	for i := 0; i < 0xFFFFFF; i++ {
		existing, taken := na.byID[id]
		if !taken || existing == absPath {
			na.byID[id] = absPath
			return id, na
		}
		id++
		if id == 0 || id > 0xFFFFFF {
			id = 1
		}
	}
	return id, na
}

// AllocFrameIDs returns n distinct IDs for animated image frames.
func (a imageIDAllocator) AllocFrameIDs(absPath string, n int) ([]uint32, imageIDAllocator) {
	na := a.clone()
	ids := make([]uint32, 0, n)
	for i := 0; i < n; i++ {
		seed := fmt.Sprintf("%s#frame%d", absPath, i)
		id := imagekit.AllocID(seed)
		for {
			existing, taken := na.byID[id]
			if !taken || existing == absPath {
				na.byID[id] = absPath
				break
			}
			id++
			if id == 0 || id > 0xFFFFFF {
				id = 1
			}
		}
		ids = append(ids, id)
	}
	return ids, na
}

// FreeAllForPath releases every ID allocated for absPath — the base ID and
// any animation frame IDs — so a despawn (M6, syncImageSet) lets a later
// respawn or an unrelated image reuse them instead of leaking them for the
// rest of the session.
func (a imageIDAllocator) FreeAllForPath(absPath string) imageIDAllocator {
	na := a.clone()
	for id, p := range na.byID {
		if p == absPath {
			delete(na.byID, id)
		}
	}
	return na
}
