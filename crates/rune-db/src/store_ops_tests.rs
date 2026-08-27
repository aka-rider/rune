//! Tests for `store_ops.rs`'s domain-verb convenience methods — split out
//! to keep the parent under the file-size ceiling.
//!
//! Every method here is a thin `enqueue` wrapper whose entire job is to
//! hand back the fresh op id `Store::enqueue` minted for correlating the
//! eventual `DbEvent`. A mutant that collapses one of these methods to a
//! hardcoded `Ok(0)`/`Ok(1)` still type-checks and still "enqueues" nothing
//! wrong per se — the only thing it breaks is that returned id. So every
//! test here calls its method TWICE in a row on the same fresh `Store` and
//! asserts the second id is exactly one more than the first: a hardcoded
//! constant returns the SAME id both times, which fails that assertion
//! regardless of which absolute id number the real sequence happens to be
//! at (no dependency on `next_op_id`'s starting value, so this stays
//! correct even if construction order elsewhere in this file changes).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use rune_vfs::{Mem, Sighting, Stat, Vfs};

use super::*;
use crate::ids::{DocId, ObsId};
use crate::merge_state::MergeCloseState;
use crate::store::ClockFn;
use crate::writer::OnEvent;

fn test_vfs() -> Arc<dyn Vfs + Send + Sync> {
    Arc::new(Mem::new())
}

fn noop_on_event() -> OnEvent {
    Box::new(|_evt| {})
}

fn open_store() -> Store {
    let clock: ClockFn = Arc::new(SystemTime::now);
    Store::open_in_memory(clock, test_vfs(), noop_on_event()).expect("open in-memory store")
}

/// Publishes `bytes` at `path` in `vfs` — `write_durable` alone only lands
/// the bytes at its own sibling temp path, exactly like `load_tests.rs`'s
/// own `publish` helper.
fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// A real `Stat`, read back from a freshly published file in a standalone
/// `Mem` — `rename_replace`'s `seen: Stat` argument only needs to be
/// well-formed, not tied to the store's own vfs, since these tests only
/// observe the returned op id, never the writer thread's eventual disk
/// work.
fn any_stat() -> Stat {
    let vfs = Mem::new();
    let path = Path::new("/any.md");
    publish(&vfs, path, b"hello");
    vfs.stat(path).expect("stat seeded file")
}

/// A real `Sighting`, read back the same way `load_sighted`'s real callers
/// obtain one (`rune_vfs::get`).
fn any_sighting() -> Sighting {
    let vfs = Mem::new();
    let path = Path::new("/any.md");
    publish(&vfs, path, b"hello");
    rune_vfs::get(&vfs, path, rune_vfs::MAX_DOCUMENT_BYTES).expect("get seeded file")
}

fn assert_ids_are_consecutive(first: u64, second: u64) {
    assert_eq!(
        second,
        first + 1,
        "op ids must be consecutive: first={first}, second={second}"
    );
}

#[test]
fn touch_search_query_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store.touch_search_query("q").expect("enqueue 1");
    let b = store.touch_search_query("q").expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn touch_command_name_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store.touch_command_name("cmd").expect("enqueue 1");
    let b = store.touch_command_name("cmd").expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn probe_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store.probe(DocId(1)).expect("enqueue 1");
    let b = store.probe(DocId(1)).expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn merge_open_returns_consecutive_op_ids() {
    let store = open_store();
    let theirs = ObsId::new(1).expect("nonzero");
    let a = store
        .merge_open(DocId(1), None, theirs, "marker", "[]")
        .expect("enqueue 1");
    let b = store
        .merge_open(DocId(1), None, theirs, "marker", "[]")
        .expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn merge_progress_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store
        .merge_progress(DocId(1), "marker", "[]")
        .expect("enqueue 1");
    let b = store
        .merge_progress(DocId(1), "marker", "[]")
        .expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn merge_close_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store
        .merge_close(DocId(1), MergeCloseState::Abandoned)
        .expect("enqueue 1");
    let b = store
        .merge_close(DocId(1), MergeCloseState::Abandoned)
        .expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn rename_file_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store
        .rename_file(DocId(1), Path::new("/a.md"), Path::new("/b.md"))
        .expect("enqueue 1");
    let b = store
        .rename_file(DocId(1), Path::new("/a.md"), Path::new("/b.md"))
        .expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn rename_replace_returns_consecutive_op_ids() {
    let store = open_store();
    let seen = any_stat();
    let a = store
        .rename_replace(DocId(1), Path::new("/a.md"), Path::new("/b.md"), seen)
        .expect("enqueue 1");
    let b = store
        .rename_replace(DocId(1), Path::new("/a.md"), Path::new("/b.md"), seen)
        .expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn load_sighted_returns_consecutive_op_ids() {
    let store = open_store();
    let path = Path::new("/a.md");
    let a = store.load_sighted(path, any_sighting()).expect("enqueue 1");
    let b = store.load_sighted(path, any_sighting()).expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn resolve_adopt_returns_consecutive_op_ids() {
    let store = open_store();
    let obs = ObsId::new(1).expect("nonzero");
    let a = store.resolve_adopt(DocId(1), obs, None).expect("enqueue 1");
    let b = store.resolve_adopt(DocId(1), obs, None).expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn resolve_abandon_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store.resolve_abandon(DocId(1)).expect("enqueue 1");
    let b = store.resolve_abandon(DocId(1)).expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn create_named_scratch_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store
        .create_named_scratch("/intended.md")
        .expect("enqueue 1");
    let b = store
        .create_named_scratch("/intended.md")
        .expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn recoverable_scratch_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store.recoverable_scratch(1).expect("enqueue 1");
    let b = store.recoverable_scratch(1).expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn find_named_scratch_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store.find_named_scratch("/intended.md").expect("enqueue 1");
    let b = store.find_named_scratch("/intended.md").expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}

#[test]
fn reconstruct_scratch_returns_consecutive_op_ids() {
    let store = open_store();
    let a = store.reconstruct_scratch(DocId(1)).expect("enqueue 1");
    let b = store.reconstruct_scratch(DocId(1)).expect("enqueue 2");
    assert_ids_are_consecutive(a, b);
    store.shutdown();
}
