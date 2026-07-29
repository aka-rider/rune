# TODO: `bind_new_now` has no dead-writer fallback

WP7 gave `save::materialize_now` (the overwrite path — an already-bound
document's ⌘S) a direct-vfs fallback for when `MaterializePrepare`'s enqueue
fails because the writer thread is confirmed gone: the store degrades, but
the user's bytes still reach disk via the same uncoordinated
`vfs.save_atomic` route a document with no store binding at all already
uses.

`save::bind_new_now` (the draft-naming route, ^R -> Enter on an untitled
document) does NOT get an equivalent fallback. A plain `vfs.save_atomic` has
no no-clobber guarantee (bind-new's whole point is an atomic `rename_excl`
create), and the existing no-store completion path
(`handle_save_done`/`Msg::SaveDone`) never binds `Document::file_path` —
only `handle_materialize_ack`'s `pending_bind_path` dance does that. Reusing
`save_cmd` here would silently create the file without ever giving the
draft its name.

Today, on a dead writer, naming a draft just degrades the store and leaves
the draft unbound (the in-memory buffer is never at risk — nothing is lost,
the user can retry once available). A proper fix would give the create path
its own no-clobber `Cmd` (mirroring `rename.rs`'s no-store `bind_new`
branch, which already does this for a document with no store at all) and
route its ack through the same `Document::bind_path` chokepoint. Narrower in
scope than [rune-db 1] (an ALREADY-bound document's overwrite), which this
package (WP7) does fix.
