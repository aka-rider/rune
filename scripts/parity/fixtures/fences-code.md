Intro paragraph, cursor lands here rather than inside a fence below.

A Rust fence, expected to gain tree-sitter token colours on the Rust side:

```rust
fn main() {
    println!("hello");
}
```

A Python fence, expected to gain tree-sitter token colours on the Rust side:

```python
def greet(name):
    print(f"hello {name}")
```

A JSON fence, expected to gain tree-sitter token colours on the Rust side:

```json
{"key": "value", "n": 1}
```

An unknown language tag — must render plainly, no error:

```klingon
Qapla'
```

An untagged fence — must also render plainly, no error:

```
untagged fenced text
```

Trailing paragraph after every code example above.
