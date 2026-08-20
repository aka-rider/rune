#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconSet {
    pub headings: [&'static str; 6],
    pub bullets: [&'static str; 4],
    pub quote_bar: &'static str,
    pub rule: &'static str,
}

impl IconSet {
    pub fn nerd() -> IconSet {
        IconSet {
            headings: [
                "\u{F0CA1} ",
                "\u{F0CA3} ",
                "\u{F0CA5} ",
                "\u{F0CA7} ",
                "\u{F0CA9} ",
                "\u{F0CAB} ",
            ],
            bullets: ["\u{25CF}", "\u{25CB}", "\u{25C6}", "\u{25C7}"],
            quote_bar: "\u{258B}",
            rule: "\u{2500}",
        }
    }

    pub fn unicode() -> IconSet {
        IconSet {
            headings: [
                "\u{25C9} ",
                "\u{25C8} ",
                "\u{25C6} ",
                "\u{25C7} ",
                "\u{25B8} ",
                "\u{25B9} ",
            ],
            bullets: ["\u{2022}", "\u{25E6}", "\u{25AA}", "\u{25AB}"],
            quote_bar: "\u{258E}",
            rule: "\u{2500}",
        }
    }
}
