Intro paragraph, cursor lands here rather than inside a fence below.

A fenced code block with a language tag:

```rust
fn main() {
    println!("hello");
}
```

A fenced code block with no language tag:

```
plain fenced text
second line
```

An indented code block (four spaces), the older Markdown style:

    indented code line one
    indented code line two

Inline `code span` inside an ordinary paragraph, plus a fence below with
a long line to check wrap behaviour inside a code block:

```text
this is a single long line inside a fence to check that wrapping inside code blocks behaves sensibly
```

Trailing paragraph after every code example above.
