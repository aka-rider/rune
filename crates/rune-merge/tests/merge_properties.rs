//! Randomized invariants for `merge_hunks`: a side's own additions never
//! vanish, conflict bytes are verbatim, one-sided merges are exact, and
//! the hunk sequence keeps its canonical shape. Seeds are fixed, so every
//! run checks the identical corpus.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;

use rune_merge::{Hunk, merge_hunks};

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
}

const LINES: &[&[u8]] = &[
    b"alpha\n",
    b"beta\n",
    b"gamma\n",
    b"delta\r\n",
    b"\n",
    b"alpha\n",
];

fn random_line(rng: &mut XorShift) -> Vec<u8> {
    LINES[rng.below(LINES.len() as u64) as usize].to_vec()
}

fn random_ancestor(rng: &mut XorShift) -> Vec<Vec<u8>> {
    (0..rng.below(7)).map(|_| random_line(rng)).collect()
}

fn derive_side(rng: &mut XorShift, ancestor: &[Vec<u8>]) -> Vec<u8> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for line in ancestor {
        match rng.below(10) {
            0 | 1 => {}
            2 | 3 => lines.push(random_line(rng)),
            _ => lines.push(line.clone()),
        }
    }
    for _ in 0..rng.below(3) {
        let at = rng.below(lines.len() as u64 + 1) as usize;
        lines.insert(at, random_line(rng));
    }
    let mut bytes: Vec<u8> = lines.concat();
    if rng.below(4) == 0 && bytes.ends_with(b"\n") {
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
    }
    bytes
}

fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|&b| b == b'\n')
            .map_or(rest.len(), |i| i + 1);
        let (line, remaining) = rest.split_at(end);
        lines.push(line);
        rest = remaining;
    }
    lines
}

fn line_counts(bytes: &[u8]) -> HashMap<Vec<u8>, usize> {
    let mut counts = HashMap::new();
    for line in split_lines(bytes) {
        *counts.entry(line.to_vec()).or_insert(0) += 1;
    }
    counts
}

fn view(hunks: &[Hunk], take_ours: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for h in hunks {
        match h {
            Hunk::Clean(bytes) => out.extend_from_slice(bytes),
            Hunk::Conflict { ours, theirs } => {
                out.extend_from_slice(if take_ours { ours } else { theirs });
            }
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    let mut count = 0;
    let mut at = 0;
    while at + needle.len() <= haystack.len() {
        if haystack.get(at..at + needle.len()) == Some(needle) {
            count += 1;
            at += needle.len();
        } else {
            at += 1;
        }
    }
    count
}

// An unterminated final line legitimately concatenates with an insertion
// that lands after it (diff3's no-trailing-newline hazard), so survival is
// checked as substring occurrences, not as intact lines.
fn assert_additions_survive(case: &str, side: &[u8], ancestor: &[u8], side_view: &[u8]) {
    let anc = line_counts(ancestor);
    for (line, have) in line_counts(side) {
        let inherited = anc.get(&line).copied().unwrap_or(0);
        if have > inherited {
            let added = have - inherited;
            let visible = occurrences(side_view, &line);
            assert!(
                visible >= added,
                "{case}: side-added line {line:?} vanished: added {added}, visible {visible}"
            );
        }
    }
}

fn assert_shape(case: &str, hunks: &[Hunk]) {
    assert!(!hunks.is_empty(), "{case}: hunks must never be empty");
    for pair in hunks.windows(2) {
        assert!(
            !matches!(pair, [Hunk::Clean(_), Hunk::Clean(_)]),
            "{case}: adjacent Clean hunks must coalesce"
        );
    }
    for h in hunks {
        if let Hunk::Conflict { ours, theirs } = h {
            assert!(
                !(ours.is_empty() && theirs.is_empty()),
                "{case}: a conflict with both sides empty is meaningless"
            );
        }
    }
    if hunks.len() > 1 {
        for h in hunks {
            if let Hunk::Clean(bytes) = h {
                assert!(!bytes.is_empty(), "{case}: empty Clean amid other hunks");
            }
        }
    }
}

#[test]
fn random_merges_preserve_both_sides_additions_and_shape() {
    let mut rng = XorShift(0x5eed_cafe_f00d_0001);
    for case_no in 0..3000 {
        let ancestor: Vec<u8> = random_ancestor(&mut rng).concat();
        let ours = derive_side(
            &mut rng,
            &split_lines(&ancestor)
                .iter()
                .map(|l| l.to_vec())
                .collect::<Vec<_>>(),
        );
        let theirs = derive_side(
            &mut rng,
            &split_lines(&ancestor)
                .iter()
                .map(|l| l.to_vec())
                .collect::<Vec<_>>(),
        );
        let case = format!("case {case_no} anc={ancestor:?} ours={ours:?} theirs={theirs:?}");

        let hunks = merge_hunks(&ancestor, &ours, &theirs);

        assert_shape(&case, &hunks);
        let ours_view = view(&hunks, true);
        let theirs_view = view(&hunks, false);
        assert_additions_survive(&case, &ours, &ancestor, &ours_view);
        assert_additions_survive(&case, &theirs, &ancestor, &theirs_view);
        for h in &hunks {
            if let Hunk::Conflict {
                ours: co,
                theirs: ct,
            } = h
            {
                assert!(contains(&ours, co), "{case}: conflict ours not verbatim");
                assert!(
                    contains(&theirs, ct),
                    "{case}: conflict theirs not verbatim"
                );
            }
        }
    }
}

#[test]
fn one_sided_merges_are_exact() {
    let mut rng = XorShift(0x5eed_cafe_f00d_0002);
    for case_no in 0..1500 {
        let ancestor: Vec<u8> = random_ancestor(&mut rng).concat();
        let anc_lines: Vec<Vec<u8>> = split_lines(&ancestor).iter().map(|l| l.to_vec()).collect();
        let changed = derive_side(&mut rng, &anc_lines);
        let case = format!("case {case_no} anc={ancestor:?} changed={changed:?}");

        let ours_only = merge_hunks(&ancestor, &changed, &ancestor);
        assert_eq!(
            view(&ours_only, true),
            changed,
            "{case}: ours-only merge must equal ours"
        );
        assert!(
            !ours_only.iter().any(|h| matches!(h, Hunk::Conflict { .. })),
            "{case}: ours-only merge must be conflict-free"
        );

        let theirs_only = merge_hunks(&ancestor, &ancestor, &changed);
        assert_eq!(
            view(&theirs_only, true),
            changed,
            "{case}: theirs-only merge must equal theirs"
        );
        assert!(
            !theirs_only
                .iter()
                .any(|h| matches!(h, Hunk::Conflict { .. })),
            "{case}: theirs-only merge must be conflict-free"
        );

        let agreeing = merge_hunks(&ancestor, &changed, &changed);
        assert_eq!(
            view(&agreeing, true),
            changed,
            "{case}: agreeing sides must merge to their shared content"
        );
        assert!(
            !agreeing.iter().any(|h| matches!(h, Hunk::Conflict { .. })),
            "{case}: agreeing sides must be conflict-free"
        );
    }
}
