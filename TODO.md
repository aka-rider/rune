# TODO

## CLIP-OSC52 fails on cut inside reading view (found by `make test-fuzz`)

`make test-fuzz` (the `human_session` fuzzer) deterministically reproduces a `CLIP-OSC52`
violation, unrelated to the `SCROLL-IN-DOC` invariant work that surfaced it (that work only
added a new, independent check; this failure trips an existing, older invariant). Minimal
repro script (see `crates/rune-fuzz/artifacts/proptest-regressions/human_session.txt`):

```
type hello world
diverge-disk
deliver-db-all
key char:m ctrl        (merge chord)
deliver-db-all
key char:y shift+sup    (redo? — see MergeState)
key char:m ctrl
deliver
key right shift          (extend selection)
key char:P ctrl           (toggle reading view — ReadOnly::Reading)
key char:x sup             (cmd+x, cut)
```

Panic: `CLIP-OSC52: no OSC 52 raw chunk decoded to the selected text "f"; raw chunks emitted: 0`

Cutting (cmd+x) a selection while the document is in reading view (read-only) emits no OSC 52
clipboard chunk at all — not even a copy-only fallback. Needs a real fix: either cut in a
read-only document should still copy the selection to the clipboard (just skip the delete), or
the fuzzer's own `CLIP-OSC52` invariant needs a documented read-only carve-out if that refusal
is intentional product behavior. Not investigated further — out of scope for the viewport-clamp
fix this TODO was filed alongside.
