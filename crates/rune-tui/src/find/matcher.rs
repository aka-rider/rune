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
    literal_replacement: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatternError(pub String);

impl Matcher {
    pub(crate) fn compile(query: &str, options: MatchOptions) -> Result<Matcher, PatternError> {
        let literal_replacement = !options.regex;
        if query.trim().is_empty() {
            return Ok(Matcher {
                pattern: None,
                literal_replacement,
            });
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
            literal_replacement,
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

    pub(crate) fn replacements(&self, hay: &str, replacement: &str) -> Vec<(Range<usize>, String)> {
        self.pattern
            .iter()
            .flat_map(|pattern| pattern.captures_iter(hay))
            .filter_map(|captures| {
                let range = captures.get(0)?.range();
                (!range.is_empty()).then(|| (range, self.expand(&captures, replacement)))
            })
            .collect()
    }

    pub(crate) fn replacement_at(
        &self,
        hay: &str,
        range: &Range<usize>,
        replacement: &str,
    ) -> Option<String> {
        let captures = self.pattern.as_ref()?.captures_at(hay, range.start)?;
        (captures.get(0)?.range() == *range).then(|| self.expand(&captures, replacement))
    }

    fn expand(&self, captures: &regex::Captures<'_>, replacement: &str) -> String {
        if self.literal_replacement {
            return replacement.to_string();
        }
        let mut expanded = String::new();
        captures.expand(replacement, &mut expanded);
        expanded
    }
}
