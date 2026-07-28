Intro paragraph in plain ASCII before any wide characters appear below.

A line of Chinese text: 你好，世界，这是一个测试。

A line of Japanese text: こんにちは、世界。これはテストです。

A line of Korean text: 안녕하세요, 세계. 이것은 테스트입니다.

Mixed ASCII and CJK on one line: hello 世界 world 你好 mixed width.

**Bold CJK** text: **你好世界** should still be double-width per glyph.

## 标题文字

- 列表项目一
- 列表项目二

A long CJK line to check wrapping at a narrow viewport width when every
glyph counts for two columns instead of one: 这一行文字足够长应该会换行。

Trailing paragraph after every CJK example above.
