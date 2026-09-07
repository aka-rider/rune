use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MatchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Matcher {
    pattern: Option<regex::Regex>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatternError(pub String);

impl Matcher {
    pub(crate) fn compile(query: &str, options: MatchOptions) -> Result<Matcher, PatternError> {
        if query.trim().is_empty() {
            return Ok(Matcher { pattern: None });
        }
        let mut source = if options.regex {
            query.to_string()
        } else {
            regex::escape(query)
        };
        if options.whole_word {
            source = format!(r"\b(?:{source})\b");
        }
        if !options.case_sensitive {
            source.insert_str(0, "(?i)");
        }
        let pattern = regex::Regex::new(&source).map_err(|e| PatternError(e.to_string()))?;
        Ok(Matcher {
            pattern: Some(pattern),
        })
    }

    pub(crate) fn hits(&self, hay: &str) -> Vec<Range<usize>> {
        self.pattern
            .iter()
            .flat_map(|pattern| pattern.find_iter(hay))
            .map(|found| found.range())
            .filter(|range| !range.is_empty())
            .collect()
    }
}
