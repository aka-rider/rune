#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Write;
use std::time::Instant;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_md::snapshot::{DisplaySnapshot, ImageDims};
use rune_syntax::wrap::WrapMap;

const WIDTH: u16 = 100;

fn build_prose_doc(target_bytes: usize) -> String {
    let line = "The quick brown fox jumps over the lazy dog near the riverbank at dawn.\n";
    let mut doc = String::with_capacity(target_bytes + line.len());
    while doc.len() < target_bytes {
        doc.push_str(line);
    }
    doc
}

fn build_short_lines_doc(target_bytes: usize) -> String {
    let line = "short line here\n";
    let mut doc = String::with_capacity(target_bytes + line.len());
    while doc.len() < target_bytes {
        doc.push_str(line);
    }
    doc
}

fn build_long_lines_doc(target_bytes: usize) -> String {
    let word = "lorem ";
    let mut single_line = String::new();
    while single_line.len() < 20_000 {
        single_line.push_str(word);
    }
    single_line.push('\n');
    let mut doc = String::with_capacity(target_bytes + single_line.len());
    while doc.len() < target_bytes {
        doc.push_str(&single_line);
    }
    doc
}

fn build_span_dense_doc(target_bytes: usize) -> String {
    let line = "A **bold** and *italic* [link](https://example.com/x) with `code` and **more bold** text here.\n";
    let mut doc = String::with_capacity(target_bytes + line.len());
    while doc.len() < target_bytes {
        doc.push_str(line);
    }
    doc
}

fn build_span_dense_long_line_doc(target_bytes: usize) -> String {
    let chunk = "**bold** *italic* [link](https://example.com/x) `code` ~~strike~~ ";
    let mut single_line = String::new();
    while single_line.len() < 20_000 {
        single_line.push_str(chunk);
    }
    single_line.push('\n');
    let mut doc = String::with_capacity(target_bytes + single_line.len());
    while doc.len() < target_bytes {
        doc.push_str(&single_line);
    }
    doc
}

struct Timing {
    parse: f64,
    sync_content: f64,
    emit_with: f64,
    wrap_sync: f64,
    from_wrap: f64,
    expand_tables: f64,
    expand_images: f64,
    snapshot_total: f64,
}

fn measure(content: &str) -> Timing {
    let buf = Buffer::new(content);

    let t0 = Instant::now();
    let blocks = rune_md::parse::parse(content);
    let parse = t0.elapsed().as_secs_f64() * 1000.0;

    let mut machine = DocMachine::new();
    let t1 = Instant::now();
    machine.sync_content(&buf);
    let sync_content = t1.elapsed().as_secs_f64() * 1000.0;
    machine.set_width(WIDTH);
    let cursors = CursorSet::new(0);
    machine.sync_cursors(&buf, &cursors, &[]);

    let t2 = Instant::now();
    let _snap = machine.snapshot(&buf);
    let snapshot_total = t2.elapsed().as_secs_f64() * 1000.0;

    let t3 = Instant::now();
    let (lines, _syntax) = rune_md::emit::emit(content, &blocks, WIDTH);
    let emit_with = t3.elapsed().as_secs_f64() * 1000.0;

    let t4 = Instant::now();
    let wrap = WrapMap::new(WIDTH).sync(content, &lines);
    let wrap_sync = t4.elapsed().as_secs_f64() * 1000.0;

    let t5 = Instant::now();
    let display = DisplaySnapshot::from_wrap(&wrap);
    let from_wrap = t5.elapsed().as_secs_f64() * 1000.0;

    let t6 = Instant::now();
    let display = display.expand_tables(&wrap);
    let expand_tables = t6.elapsed().as_secs_f64() * 1000.0;

    let images = ImageDims::new();
    let t7 = Instant::now();
    let _display = display.expand_images(&wrap, &blocks, content, &images);
    let expand_images = t7.elapsed().as_secs_f64() * 1000.0;

    Timing {
        parse,
        sync_content,
        emit_with,
        wrap_sync,
        from_wrap,
        expand_tables,
        expand_images,
        snapshot_total,
    }
}

fn run_shape(name: &str, builder: fn(usize) -> String) {
    println!("\n=== shape: {name} ===");
    println!(
        "{:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "bytes",
        "parse",
        "sync_ctnt",
        "emit_with",
        "wrap_sync",
        "from_wrap",
        "exp_tbls",
        "exp_imgs",
        "snap_tot"
    );
    for target_mb in [0.5_f64, 1.0, 2.0, 5.0] {
        let target_bytes = (target_mb * 1_000_000.0) as usize;
        let content = builder(target_bytes);
        let t = measure(&content);
        println!(
            "{:>8.1}MB {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2}",
            content.len() as f64 / 1_000_000.0,
            t.parse,
            t.sync_content,
            t.emit_with,
            t.wrap_sync,
            t.from_wrap,
            t.expand_tables,
            t.expand_images,
            t.snapshot_total,
        );
        let _ = std::io::stdout().flush();
    }
}

fn main() {
    run_shape("plain_prose", build_prose_doc);
    run_shape("many_short_lines", build_short_lines_doc);
    run_shape("few_long_lines", build_long_lines_doc);
    run_shape("span_dense_short_lines", build_span_dense_doc);
    run_shape("span_dense_long_lines", build_span_dense_long_line_doc);
}
