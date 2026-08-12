# The Rune Constitution

This is the law — read it before contributing anything, not only persistence or UI code. Every rule below is binding, not advisory. When code and this document disagree, one of them is wrong; fix it in the change that found it.

---

## 1. Prime Directive

Never corrupt, never lose what the user wrote. When data safety conflicts with performance, elegance, or features, data safety wins.

If it can be a compile error, it MUST be a compile error.

Rank every defect by the harm it can do, and always trade a failure down:

1. **Catastrophic — silent corruption.** Wrong/garbled/reordered bytes, a silent rewrite, a good file overwritten by a bad buffer. Never ships.
2. **Severe — losing work.** A crash, failed save, or botched recovery that discards unsaved edits.
3. **Tolerable — everything else.** A render glitch, a dropped keypress, a clean halt that loses nothing.

Prefer a Tolerable halt — a surfaced error that keeps the buffer — over any higher rung; never panic — a panic takes the unsaved buffer with it.

## 2. I/O

- All disk access for user files goes through the one injected `Vfs`, constructed once in `main` and threaded everywhere; rune-db's own database sidecar file is the sole exception.
- User content reaches disk only through a durable temp write followed by exactly one atomic publish (exchange/rename); nothing else writes the destination.
- A durability failure discovered after the publish already took effect is physical success — report it as such, and never remove the temp that still holds the displaced bytes.
- Bytes a write displaces are captured as a durable blob before anything discards them.
- Unsaved work goes to the recovery store, never to the user's file.
- Losing the database must never damage the user's file — it is an observer beside the file, never inside it.

## 3. Bytes

- Load, edit, save is byte-identical outside the user's own edits — no normalization: line endings, trailing newline, BOM, and encoding all pass through verbatim.
- Invalid UTF-8 is refused at load, never repaired.
- Edit, cursor, and journal offsets are bytes; display width is measured in terminal cells over whole grapheme clusters, through one width chokepoint — never a byte count, never a char count.
- Refuse, don't guess, at the buffer boundary; clamp only at the caller's boundary.
- A destructive async replacement is suspect until proven — an empty reset is never a user deletion.
- Dirty is a content comparison, re-derived at every decision point — never a cached flag trusted for a decision.

## 4. No Panic

- The workspace's panic/unwrap/expect lints are law; never `allow` them in production code.
- `assert!`/`debug_assert!` evade those lints — a "can't happen" check routes through the `assert_invariant!` macro, armed by tests or a crate's `strict-invariants` feature, never by `cfg!(debug_assertions)`.
- A refusal is a typed return value, never a swallowed `Result`.
- `panic = "abort"` is forbidden — terminal restore runs on unwind.
- A crash in linked C is not a Rust panic, and no lint can see it: never construct a tree-sitter `InputEdit`; every parse is a full parse.

## 5. Update Cycle

- `update` is the sole writer of synchronous state; a `Cmd` exists only for work that leaves the thread.
- Render is a pure function of `&App`.
- A stale async reply is killed by a generation or version echo carried on the request, never by resolving live state on arrival.
- A timeout is a message, never a sleep inside `update`.
- Wall-clock time is read only through the injected `Clock`.

## 6. Dispatch

- No ad-hoc keystroke bindings — const tables resolved by the one resolver.
- Every printable global chord requires ctrl or sup.
- Modal capture is total; a mode that captures the keyboard consumes every key with visible feedback.
- Every user action gets feedback — silent input swallowing is a defect, not a shortcut.

## 7. State

- No shadow state: every fact has one writer; derived state is re-derived from its source, never cached and read back as truth.
- Per-editing-pane state lives on `Document`; only genuinely app-wide state lives on `App`.
- Disk facts update only from an operation's own result, never from a watcher or a poll.

## 8. Display & Layout

- One geometry chokepoint; one snapshot producer per document.
- Tree-sitter output is a render overlay, never an emitted span.
- Decoration is metadata, never a text mutation.
- No viewport-keyed caches — a retained parse is re-queried per frame, never cached per scroll position.

## 9. Style

- Formatting is rustfmt's, never hand-argued.
- The workspace's clippy lints are law.
- Naming and interface design follow the Rust API Guidelines.
- If a signature needs prose to be understood, redesign the signature.
- Make illegal states unrepresentable: an enum over a sentinel, `Option` over a magic value, a newtype over a raw scalar shared by two meanings.

## 10. Comments

Never write code comments, rustdoc included — rune is an application, not a library; there is no downstream API consumer, and rustdoc rots exactly like any other comment.

Rare, exhaustive exceptions: a complex algorithm explained inside the function that implements it; a third-party quirk that saves the next person real debugging time; a constraint that no type, name, or test can carry.

Never TODO or HACK a comment — fix it now, or ledger it in `TODO.md`. Never cite a file path or a `path:line` — either rots the moment the file moves; a bare name is search fodder, not a substitute for stating the invariant. Never note what calls or is called — the reader has search. A comment restating what the code does is rot the moment the code changes; a paragraph justifying why a workaround is okay indicts the code — refactor until the comment is unnecessary.

## 11. Visual Language

Before adding any style or color, read the existing styles. One meaning per color or format across the whole app: reuse what already communicates a meaning; reconcile any clash rather than let one color mean two things.

## 12. Testing

- Behavior is tested through the real update seam, with real messages — never by poking the state a behavior is supposed to produce.
- Fixtures set preconditions, never the asserted outcome.
- Time is an injected field — no wall-clock sleeps order events anywhere.
- Every invariant claim ships as a test that can fail; a gate that cannot fail proves nothing.
- A fuzz catch lands as a pinned repro in the same commit as its fix.

## 13. The Ledger

`TODO.md` at the repo root is the refactor ledger: known violations of these rules are inventoried there and must not spread. New code follows every rule above even where neighboring legacy code does not.
