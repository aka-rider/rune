# Heading One — Project Rune

Intro paragraph with **bold**, *italic*, ***bold italic***, `inline code`, ~~strikethrough~~, and a [link](https://example.com).

## Heading Two — Features

### Heading Three

#### Heading Four

##### Heading Five

###### Heading Six

- Bullet item one
- Bullet item two with `code`
  - Nested bullet
    - Deeper nested
- [ ] Unchecked task
- [x] Checked task

1. Ordered first
2. Ordered second
   1. Nested ordered

> A blockquote line with **bold** inside.
> Second quote line.
>
> > Nested quote.

Term paragraph before a thematic break.

---

```rust
fn main() {
    let greeting = "hello";
    println!("{greeting}, world"); // comment
}
```

```markdown
# A heading inside a markdown fence

Some **bold** and *italic* and `code` inside the fence.

- a list item
```

```python
def add(a: int, b: int) -> int:
    return a + b  # simple
```

```
plain fence with no language
```

| Column A | Column B | Column C |
|----------|---------:|:--------:|
| left     |    right | center   |
| a        |        1 | x        |

Inline HTML: <em>emphasis</em> and an image: ![alt text](image.png)

Final paragraph with a footnote-like ref [^1] and an autolink <https://rune.dev>.

[^1]: The footnote text.
