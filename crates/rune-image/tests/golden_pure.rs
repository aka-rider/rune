//! Byte-parity: every pure function in `rune-image` reproduces the
//! committed golden expectations exactly, verified against
//! `tests/golden/pure.json` (frozen expected values, updated deliberately
//! by editing the JSON).
//!
//! Parsed with `serde_json::Value` rather than a `#[derive(Deserialize)]`
//! struct so this crate does not need to add `serde` on top of the
//! already-shared `serde_json`.
#![allow(clippy::panic)]

use serde_json::Value;
use std::fs;

use rune_image::{CellSize, alloc_id, clamp_delay, diacritic, fit_box, fit_cells};

fn load_golden() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/pure.json");
    let text = fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "read {path}: {e} (goldens are committed expectations under tests/golden/; \
             update them deliberately by editing the JSON)"
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn as_usize(v: &Value, key: &str) -> usize {
    v.get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("missing/non-integer field {key:?} in {v}")) as usize
}

fn as_i64(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing/non-integer field {key:?} in {v}"))
}

fn as_str<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing/non-string field {key:?} in {v}"))
}

fn cases<'a>(golden: &'a Value, key: &str) -> &'a Vec<Value> {
    golden
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("missing array field {key:?}"))
}

#[test]
fn fit_box_matches_golden() {
    let golden = load_golden();
    let list = cases(&golden, "fit_box");
    assert!(!list.is_empty());
    for case in list {
        let input = &case["input"];
        let output = &case["output"];
        let (w, h) = fit_box(
            as_usize(input, "src_w"),
            as_usize(input, "src_h"),
            as_usize(input, "max_w"),
            as_usize(input, "max_h"),
        );
        assert_eq!(
            (w, h),
            (as_usize(output, "w"), as_usize(output, "h")),
            "fit_box({input})"
        );
    }
}

#[test]
fn fit_cells_matches_golden() {
    let golden = load_golden();
    let list = cases(&golden, "fit_cells");
    assert!(!list.is_empty());
    for case in list {
        let input = &case["input"];
        let output = &case["output"];
        let cs = CellSize {
            w: as_usize(input, "cell_w"),
            h: as_usize(input, "cell_h"),
        };
        let (cols, rows) = fit_cells(
            as_usize(input, "px_w"),
            as_usize(input, "px_h"),
            as_usize(input, "max_cols"),
            as_usize(input, "max_rows"),
            cs,
        );
        assert_eq!(
            (cols, rows),
            (as_usize(output, "cols"), as_usize(output, "rows")),
            "fit_cells({input})"
        );
    }
}

#[test]
fn alloc_id_matches_golden() {
    let golden = load_golden();
    let list = cases(&golden, "alloc_id");
    assert!(!list.is_empty());
    for case in list {
        let input = &case["input"];
        let output = &case["output"];
        let path = as_str(input, "path");
        let want = as_i64(output, "id");
        assert_eq!(
            i64::from(alloc_id(path.as_bytes())),
            want,
            "alloc_id({path:?})"
        );
    }
}

#[test]
fn clamp_delay_matches_golden() {
    let golden = load_golden();
    let list = cases(&golden, "clamp_delay");
    assert!(!list.is_empty());
    for case in list {
        let input = &case["input"];
        let output = &case["output"];
        let hundredths = as_i64(input, "hundredths");
        let want = as_i64(output, "millis");
        let got = i64::try_from(clamp_delay(hundredths).as_millis())
            .unwrap_or_else(|_| panic!("delay overflowed i64 millis"));
        assert_eq!(got, want, "clamp_delay({hundredths})");
    }
}

#[test]
fn diacritic_matches_golden() {
    let golden = load_golden();
    let list = cases(&golden, "diacritic");
    assert!(!list.is_empty());
    for case in list {
        let input = &case["input"];
        let output = &case["output"];
        let idx = as_i64(input, "index");
        let want = as_i64(output, "codepoint");
        // `diacritic` takes an unsigned index; the golden fixture's
        // negative-index case degrades to the same table[0] fallback as
        // any other out-of-range index, which any out-of-range `usize`
        // reaches too.
        let got_idx = usize::try_from(idx).unwrap_or(usize::MAX);
        let got = i64::from(u32::from(diacritic(got_idx)));
        assert_eq!(got, want, "diacritic({idx})");
    }
}
