//! The liveness-aware marker wait's fail-fast path: a child that dies
//! before ever touching its marker must be reported immediately, by its
//! own distinct message, never mistaken for a timeout.

use crate::support::{MARKER_SAFETY_DEADLINE, spawn_helper, temp_dir, wait_ready_or_child_death};

// ---------------------------------------------------------------------
// Scenario (j): the liveness-aware marker wait fails fast on a dead child
// ---------------------------------------------------------------------

#[test]
fn wait_ready_or_child_death_panics_immediately_when_a_child_dies_before_its_marker() {
    let dir = temp_dir("fail-fast");
    let marker = dir.join("never-touched");

    let mut children = vec![spawn_helper("defunct", &[])];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_ready_or_child_death(
            &mut children,
            std::slice::from_ref(&marker),
            MARKER_SAFETY_DEADLINE,
        );
    }));

    let payload = result.expect_err("an unknown role must exit without ever touching its marker");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .expect("panic payload must be a string message");

    assert!(
        message.contains("child exited"),
        "a dead child must be reported by its own distinct message: {message}"
    );
    assert!(
        !message.contains("timed out"),
        "a dead child must never be reported as a timeout: {message}"
    );
    assert!(
        message.contains("unknown role defunct"),
        "the panic message must carry the dead child's captured stderr: {message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
