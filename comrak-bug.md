# Drafted comrak bug reports

This file holds two comrak defect reports, written to be pasted as-is into
GitHub issues at `github.com/kivikakk/comrak`. **They have not been filed.**
Filing is a separate, deliberate action the maintainer of this repo takes
under their own GitHub identity — do not post either report on their
behalf.

Both defects are in `comrak` 0.54.0's inline `Sourcepos` tracking. `rune`
consumes `Sourcepos` directly to place its highlight and conceal overlays,
so a wrong column becomes a wrong character on screen. Report 1 already has
a workaround shipped in `crates/rune-md/src/parse/shadow.rs` — it expands
leading tabs to spaces in the container-prefix region before handing text
to comrak, sidestepping the defect rather than fixing it. Report 2 has no
workaround; nothing in `rune-md` currently corrects for it, so this file is
the only record of the defect until it is filed and fixed upstream.

Both mechanism claims below were checked line-by-line against the vendored
`comrak-0.54.0` source (`~/.cargo/registry/src/index.crates.io-*/comrak-0.54.0/src/`)
before this file was written. Report 2's suggested-fix section originally
mis-stated which functions call the buggy code path; that has been
corrected below (see the "Callers" list) — the code paths that matter for
the reported symptom are `handle_backticks`, `handle_dollars`, and
`handle_latex_math`, not `handle_pointy_brace`.

Each report is self-contained and assumes no knowledge of `rune`. File them
as two separate issues.

---

## Report 1: Inline sourcepos columns are shifted right when a container consumes part of a tab

**Not found already reported.** Searched `kivikakk/comrak` issues (titles
and bodies) for `tab`, `tab stop`, `chars_to_tab`, `partially_consumed`,
`indent`, and `sourcepos tab`; none of the results describe this. The
closest neighbor, #591 ("Table sourcepos affected by indentation of
preceding line"), is a different defect — it's about a table's sourcepos
shifting with a *preceding* line's leading spaces, not about tabs.

comrak documents `Sourcepos` columns as 1-based UTF-8 byte columns. On one
kind of line they are not. When a container prefix — a list item's
indentation, a block quote's marker — consumes only part of a leading tab,
every inline node on that line reports a column one to three positions too
far right.

### Reproduction

Self-contained; add `comrak = "0.54"` to a fresh `cargo new`'s
`Cargo.toml` and run:

```rust
use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document, Options};

fn main() {
    let src = "-\n\t>d\n\tabc";
    let arena = Arena::new();
    let root = parse_document(&arena, src, &Options::default());

    let text = root
        .descendants()
        .find(|n| matches!(&n.data.borrow().value, NodeValue::Text(t) if t == "abc"))
        .unwrap();

    println!("{}", text.data.borrow().sourcepos.start.column);
}
```

The third line, `"\tabc"`, holds a tab at byte 0 and `abc` at bytes 1..4, so
`a` is at byte column 2.

- **Expected**: `2`
- **Actual**: `4`

The shift is 2 here and is bounded by the tab's width, so it is at most 3.
It scales with how much of the tab the container consumed, a quantity the
`Sourcepos` does not carry — a consumer holding only a line and a column
cannot correct for it after the fact.

### Version and environment

comrak 0.54.0, default `Options` (no extensions enabled). Not
platform-specific; reproduces on the tested toolchain (rustc via the
crate's `rust-version = "1.85"`, edition 2024).

### Mechanism

Three steps, in `src/parser/mod.rs` and `src/parser/inlines.rs`:

1. `advance_offset` (`src/parser/mod.rs:1778`), called with `columns =
   true`, detects when the requested column count ends partway through a
   tab and sets a flag without advancing past the tab:

   ```rust
   // mod.rs:1783-1790
   let chars_to_tab = TAB_STOP - (self.column % TAB_STOP);
   if columns {
       self.partially_consumed_tab = chars_to_tab > count;
       let chars_to_advance = min(count, chars_to_tab);
       self.column += chars_to_advance;
       if !self.partially_consumed_tab {
           self.offset += 1;
       };
   ```

   `self.offset` is left pointing at the tab byte itself.

2. `add_line` (`src/parser/mod.rs:1995`) then advances past that tab and
   pushes synthetic spaces into the node's `content`, but records
   `line_offsets` from the now-advanced `offset` — after the tab, with no
   record of the spaces just written:

   ```rust
   // mod.rs:1998-2010
   if self.partially_consumed_tab {
       self.offset += 1;
       let chars_to_tab = TAB_STOP - (self.column % TAB_STOP);
       ast.content.reserve(chars_to_tab);
       for _ in 0..chars_to_tab {
           ast.content.push(' ');
       }
   }
   if self.offset < line.len() {
       ast.line_offsets.push(self.offset);
       ast.content.push_str(&line[self.offset..]);
   }
   ```

3. `make_inline` (`src/parser/inlines.rs:159-162`) builds a node's column
   from an index into that same `content` buffer plus the recorded line
   offset:

   ```rust
   let start_column =
       start_column as isize + 1 + self.column_offset + self.line_offset as isize;
   ```

   The index into `content` counts the synthetic spaces `add_line` wrote;
   `line_offset` (sourced from `line_offsets[adjusted_line]` in
   `parse_inline`, `src/parser/inlines.rs:216-217`) does not subtract them.
   The result is the true byte column plus the number of synthetic spaces
   — up to 3, since a tab stop is 4 columns wide.

### Suggested fix

`line_offsets` would need to record where the line's content logically
begins counting the synthetic spaces, i.e. `self.offset - chars_to_tab` at
the `mod.rs:2010` push site. That expression can go negative when the tab
sits at the very start of the line, so `line_offsets` can't stay a
`Vec<usize>` as-is — either widen it to a signed type, or keep a parallel
per-line count of synthetic spaces and subtract it at each of the affected
read sites (inline columns in `make_inline`, the `HtmlBlock`/`HeexBlock`
end-column calculation in `finalize_borrowed`, and the table extension's
start-column fix-up all read `line_offsets`). This is offered as a
direction, not a patch — I haven't traced every call site closely enough
to be sure it's complete.

### Impact

A consumer that maps a `Sourcepos` back to its own byte buffer — a syntax
highlighter, an editor, a linter reporting a span — silently points at the
wrong bytes on these lines. Because the offset stays within the line, the
error doesn't surface as a slice failure; it surfaces as a highlight or
diagnostic drawn one to three characters off, or as a panic if the consumer
slices a UTF-8 string and the shifted offset lands mid-character.

---

## Report 2: A multi-line inline node takes its end column from the wrong line

**Not found already reported as its own issue.** The closest prior work is
#501 ("`HtmlInline` (and possibly other inlines) incorrect end line
calculation", closed) and its fix, PR #542 ("Inline sourcepos fixes.",
merged before the 0.36.0 release). That PR introduced the
`adjust_node_newlines` function and the `parent_line_offsets` lookup
described below — the mechanism this report describes is a residual bug
*in* the code #542 added, not the bug #542 fixed. I didn't find a
follow-up issue for it. Worth checking with the maintainer whether it's
already known before filing, since it sits directly downstream of recent
work in this area.

When an inline node spans a newline and does not begin on its enclosing
block's first line, its reported end column is measured against a
different line's indentation than the one it names. The error goes in both
directions.

### Reproduction

Self-contained; add `comrak = "0.54"` to a fresh `cargo new`'s
`Cargo.toml` and run:

```rust
use comrak::nodes::NodeValue;
use comrak::{Arena, parse_document, Options};

fn main() {
    for src in ["a\n`\n é`", "a\n`\n  é`", "a\n  `\né`", "a\n  `\n é`"] {
        let arena = Arena::new();
        let root = parse_document(&arena, src, &Options::default());
        let code = root
            .descendants()
            .find(|n| matches!(n.data.borrow().value, NodeValue::Code(_)))
            .unwrap();
        println!("{src:?} -> {}", code.data.borrow().sourcepos);
    }
}
```

Each input is a paragraph whose first line is `a`, followed by a code span
opened with a backtick on line 2 and closed on line 3. Only the leading
whitespace on lines 2 and 3 varies.

| document | reported end column | true end column | error |
|---|---|---|---|
| `"a\n`\n é`"` | 3 | 4 | −1 |
| `"a\n`\n  é`"` | 3 | 5 | −2 |
| `"a\n  `\né`"` | 5 | 3 | +2 |
| `"a\n  `\n é`"` | 5 | 4 | +1 |

("True end column" counted directly against the source bytes of line 3;
`é` is 2 bytes, so `` `é` `` closes at byte column 4 when line 3 is `` é`  ``
with one leading space, etc.)

The error equals the indentation of the paragraph's line at index
`last_line − code_span_start_line`, minus the indentation of the actual
last line. It vanishes whenever the span begins on the paragraph's own
first line, since those two indices coincide there — which is why a
two-line document never reproduces it. Three lines is the floor to see it.

The negative-error cases are the dangerous ones: the resulting offset still
lands inside the line, so a bounds check on the returned `Sourcepos` cannot
catch it. It just points at the wrong character.

### Version and environment

comrak 0.54.0, default `Options` (no extensions enabled). Not
platform-specific; reproduces on the tested toolchain (rustc via the
crate's `rust-version = "1.85"`, edition 2024).

### Mechanism

`adjust_node_newlines` (`src/parser/inlines.rs:2359-2380`) is called after
an inline node has already been created via `make_inline`, to correct its
end line/column for having spanned one or more newlines:

```rust
fn adjust_node_newlines(
    &mut self,
    node: Node<'a>,
    matchlen: usize,
    extra: usize,
    parent_line_offsets: &[usize],
) {
    let (newlines, since_newline) = count_newlines(...);
    if newlines > 0 {
        self.line += newlines;
        let node_ast = &mut node.data_mut();
        node_ast.sourcepos.end.line += newlines;
        let adjusted_line = self.line - node_ast.sourcepos.start.line;
        node_ast.sourcepos.end.column =
            parent_line_offsets[adjusted_line] + since_newline + extra;
        ...
    }
}
```

`parent_line_offsets` is the *block's* `line_offsets` — passed down from
the enclosing paragraph (or other block) — and it's indexed relative to
that block's own start line everywhere else it's used. The one other read
site, in `parse_inline` (`src/parser/inlines.rs:216-217`), makes this
explicit:

```rust
let adjusted_line = self.line - ast.sourcepos.start.line;
self.line_offset = ast.line_offsets[adjusted_line];
```

— here `ast` is the enclosing block, so `ast.sourcepos.start.line` is the
block's first line.

But `adjust_node_newlines` computes `adjusted_line` from
`node_ast.sourcepos.start.line` — the *inline node's own* start line, not
the parent block's. When the code span (or other multi-line inline)
doesn't begin on the block's first line, `node_ast.sourcepos.start.line` is
larger than `ast.sourcepos.start.line` by however many lines separate them,
so `adjusted_line` is short by that amount and `parent_line_offsets` is
read at the wrong index — pulling in a different line's leading-whitespace
offset. The measured errors in the table above match that account exactly:
each is the difference between the byte offset comrak records for the
*intended* last line and the byte offset it actually reads at the
shifted-by-`k` index.

The same pattern — indexing `parent_line_offsets` by the node's own start
line instead of the parent's — is repeated in
`handle_potential_attribute` (`src/parser/inlines.rs:2383-2405`, gated
behind the `attributes` feature), which has its own copy of this
calculation for attribute-suffix parsing:

```rust
ast.sourcepos.end.column =
    parent_line_offsets[ast.sourcepos.end.line - ast.sourcepos.start.line] + last_line;
```

That function is marked with its own admission of uncertainty in the
source: `// XXX I really freestyled this next line.`

**Callers.** `adjust_node_newlines` is called from `handle_backticks`
(code spans — this is the path the reproduction above exercises),
`handle_latex_math` (`\(...\)`), `handle_pointy_brace` (autolinks and raw
HTML inlines), `handle_dollars` (`$...$` math), and the HEEx paths
(`make_heex_inline`, `handle_heex_inline_expression`, behind the
`phoenix_heex` feature). Any multi-line span through any of these is
affected, not just code spans — code spans are simply the shortest way to
reproduce it. `handle_potential_attribute` is not a caller of
`adjust_node_newlines`; it's a separate function with the same bug,
reached from `handle_backticks` and `close_bracket_match` when the
`attributes` feature is enabled.

### Suggested fix

Index `parent_line_offsets` relative to the parent block's start line
rather than the node's own:

```rust
let adjusted_line = self.line - parent_start_line;
```

This needs the parent block's `sourcepos.start.line` threaded into
`adjust_node_newlines` alongside `parent_line_offsets`, since the two must
share an origin — right now only the offsets travel down, not the line
they're relative to. The same change would apply to
`handle_potential_attribute`. I haven't checked whether every caller
already has the parent's start line in scope at the call site, so this is
a direction rather than a verified patch.

### Impact

A consumer mapping a `Sourcepos` back to its own buffer gets a span whose
end position is in the wrong place — sometimes past the true end,
sometimes short of it. A downstream tool that uses the end column to
locate a closing delimiter (say, to conceal it in a rendered view, or to
splice a replacement) acts on the wrong character. The negative-error
direction is the one to worry about: the miscomputed column still falls
inside the line's byte range, so nothing about the `Sourcepos` itself looks
wrong — there is no out-of-bounds index, no panic, just a silently
mislocated span.
