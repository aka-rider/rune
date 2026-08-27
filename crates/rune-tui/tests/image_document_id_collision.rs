//! Kitty image ids are terminal-global (the Kitty graphics protocol has no
//! notion of "which document" an id belongs to), but `rune_image::alloc_id`
//! is a pure hash of the resolved path, truncated to 24 bits — two
//! documents whose paths happen to collide under that hash must still end
//! up with distinct ids, or one document's pixels silently overwrite the
//! other's on screen, and closing either one deletes both from the
//! terminal. Split out of `image_document.rs` to keep that file under the
//! repo's 500-line ceiling.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs, VfsTestExt};

const X_PNG: &[u8] = include_bytes!("../../../testdata/assets/x.png");

/// Two distinct paths whose `rune_image::alloc_id` FNV-1a hash — truncated
/// to 24 bits — happens to collide (found by brute-force search over
/// `/vault/doc{n}.png`). If the whole-document image path allocated
/// straight from the hash with no cross-document probing, both documents
/// would get the SAME id and the terminal would show one image's pixels
/// for both cells.
const COLLIDING_PATH_A: &str = "/vault/doc40829.png";
const COLLIDING_PATH_B: &str = "/vault/doc59812.png";

fn open_colliding_pair() -> (
    App,
    rune_tui::document::DocumentId,
    rune_tui::document::DocumentId,
) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new(COLLIDING_PATH_A), X_PNG)
        .expect("seed a");
    mem.save_atomic(Path::new(COLLIDING_PATH_B), X_PNG)
        .expect("seed b");
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(Buffer::new(""), None, vfs, None);
    app.graphics.kitty = true;
    let id_a = workspace::open_path(&mut app, Path::new(COLLIDING_PATH_A)).expect("open a");
    let id_b = workspace::open_path(&mut app, Path::new(COLLIDING_PATH_B)).expect("open b");
    (app, id_a, id_b)
}

#[test]
fn two_documents_whose_paths_hash_to_the_same_natural_id_still_get_distinct_kitty_ids() {
    let (app, id_a, id_b) = open_colliding_pair();
    let img_a = app.doc(id_a).unwrap().image().unwrap().id;
    let img_b = app.doc(id_b).unwrap().image().unwrap().id;
    assert_ne!(
        img_a, img_b,
        "two different documents must never share a terminal-global Kitty image id, \
         even when their paths happen to hash to the same natural id"
    );
}

#[test]
fn closing_one_colliding_document_never_deletes_the_others_kitty_id() {
    let (mut app, id_a, id_b) = open_colliding_pair();
    let img_b_before = app.doc(id_b).unwrap().image().unwrap().id;

    let mut effects = Effects::default();
    let _ = workspace::close_now(&mut app, id_a, &mut effects);

    assert!(
        app.doc(id_b).is_some(),
        "closing document A must never touch document B"
    );
    let img_b_after = app.doc(id_b).unwrap().image().unwrap().id;
    assert_eq!(
        img_b_before, img_b_after,
        "document B's id must survive document A's close untouched"
    );
    assert!(
        effects.raw_bytes().is_empty()
            || effects.raw_bytes()[0] != rune_image::encode_delete(img_b_after.get()).into_bytes(),
        "closing A must never emit a Kitty delete for B's still-live id"
    );
}
