use std::path::Path;
use std::sync::Arc;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::find::FieldState;

#[derive(Debug)]
pub(crate) struct GlobFields {
    pub include: FieldState,
    pub exclude: FieldState,
    compiled: Result<Arc<PathFilter>, GlobError>,
}

impl Default for GlobFields {
    fn default() -> GlobFields {
        let include = FieldState::default();
        let exclude = FieldState::default();
        let compiled = compile_fields(&include, &exclude);
        GlobFields {
            include,
            exclude,
            compiled,
        }
    }
}

impl GlobFields {
    pub(crate) fn refresh(&mut self) {
        self.compiled = compile_fields(&self.include, &self.exclude);
    }

    pub(crate) fn path_filter(&self) -> Result<Arc<PathFilter>, GlobError> {
        self.compiled.clone()
    }

    pub(crate) fn error(&self) -> Option<&GlobError> {
        self.compiled.as_ref().err()
    }
}

fn compile_fields(
    include: &FieldState,
    exclude: &FieldState,
) -> Result<Arc<PathFilter>, GlobError> {
    let compile = |label: &str, field: &FieldState| {
        compile_set(field.editor.text()).map_err(|e| GlobError(format!("{label}: {e}")))
    };
    Ok(Arc::new(PathFilter {
        include: compile("include", include)?,
        exclude: compile("exclude", exclude)?,
    }))
}

#[cfg(test)]
thread_local! {
    static COMPILATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn compilations() -> usize {
    COMPILATIONS.with(std::cell::Cell::get)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlobError(pub String);

#[derive(Debug)]
pub(crate) struct PathFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl PathFilter {
    pub(crate) fn admits(&self, root: &Path, path: &Path) -> bool {
        if self.include.is_none() && self.exclude.is_none() {
            return true;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        self.include
            .as_ref()
            .is_none_or(|set| set.is_match(relative))
            && !self
                .exclude
                .as_ref()
                .is_some_and(|set| set.is_match(relative))
    }
}

fn compile_set(text: &str) -> Result<Option<GlobSet>, globset::Error> {
    #[cfg(test)]
    COMPILATIONS.with(|count| count.set(count.get() + 1));
    let mut builder = GlobSetBuilder::new();
    let mut any = false;
    for pattern in split_patterns(text) {
        for source in expand(pattern) {
            builder.add(GlobBuilder::new(&source).literal_separator(true).build()?);
            any = true;
        }
    }
    any.then(|| builder.build()).transpose()
}

// Commas inside `{...}` belong to the glob's own alternation, so only a
// depth-zero comma separates two patterns.
fn split_patterns(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(text.get(start..at).unwrap_or_default());
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(text.get(start..).unwrap_or_default());
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

// A pattern names a path and everything beneath it (`docs` and `docs/` both
// cover `docs/a/b.md`); one with a `/` is anchored at the workspace root,
// one without matches at any depth.
fn expand(pattern: &str) -> Vec<String> {
    let body = pattern.trim_end_matches('/');
    let anchored = body.contains('/');
    let body = body.trim_start_matches('/');
    if body.is_empty() {
        return Vec::new();
    }
    let prefix = if anchored { "" } else { "**/" };
    vec![format!("{prefix}{body}"), format!("{prefix}{body}/**")]
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const ROOT: &str = "/root";

    fn filter(include: &str, exclude: &str) -> Arc<PathFilter> {
        let fields = fields(include, exclude);
        fields.path_filter().expect("valid globs")
    }

    fn fields(include: &str, exclude: &str) -> GlobFields {
        let mut fields = GlobFields::default();
        fields.include.editor.set_text(include);
        fields.exclude.editor.set_text(exclude);
        fields.refresh();
        fields
    }

    fn admits(filter: &PathFilter, relative: &str) -> bool {
        filter.admits(Path::new(ROOT), &Path::new(ROOT).join(relative))
    }

    #[test]
    fn empty_whitespace_and_comma_only_fields_admit_everything() {
        for text in ["", "   ", ",,", " , ,\t,"] {
            let filter = filter(text, text);
            assert!(admits(&filter, "a.md"), "{text:?}");
            assert!(admits(&filter, "x/y/z.rs"), "{text:?}");
        }
    }

    #[test]
    fn a_pattern_without_a_slash_matches_at_any_depth() {
        let filter = filter("*.md", "");
        assert!(admits(&filter, "a.md"));
        assert!(admits(&filter, "docs/deep/a.md"));
        assert!(!admits(&filter, "a.rs"));
    }

    #[test]
    fn a_pattern_with_a_slash_is_anchored_at_the_root() {
        let filter = filter("a/*.md", "");
        assert!(admits(&filter, "a/x.md"));
        assert!(!admits(&filter, "b/a/x.md"));
        assert!(!admits(&filter, "a/b/x.md"), "`*` does not cross a slash");
    }

    #[test]
    fn a_leading_slash_anchors_a_single_segment_pattern() {
        let filter = filter("/docs", "");
        assert!(admits(&filter, "docs/a.md"));
        assert!(!admits(&filter, "x/docs/a.md"));
    }

    #[test]
    fn a_bare_name_and_a_trailing_slash_cover_everything_beneath() {
        for pattern in ["docs", "docs/"] {
            let filter = filter("", pattern);
            assert!(!admits(&filter, "docs/a.md"), "{pattern}");
            assert!(!admits(&filter, "x/docs/y/a.md"), "{pattern}");
            assert!(admits(&filter, "documents/a.md"), "{pattern}");
        }
    }

    #[test]
    fn exclude_wins_over_include() {
        let filter = filter("*.md", "docs/**");
        assert!(admits(&filter, "a.md"));
        assert!(!admits(&filter, "docs/a.md"));
        assert!(!admits(&filter, "a.rs"));
    }

    #[test]
    fn entries_are_trimmed_and_several_are_alternatives() {
        let filter = filter(" *.md ,, *.rs ", "");
        assert!(admits(&filter, "a.md"));
        assert!(admits(&filter, "b.rs"));
        assert!(!admits(&filter, "c.txt"));
    }

    #[test]
    fn a_comma_inside_braces_does_not_split_the_pattern() {
        let filter = filter("*.{md,rs}", "");
        assert!(admits(&filter, "a.md"));
        assert!(admits(&filter, "b.rs"));
        assert!(!admits(&filter, "c.txt"));
    }

    #[test]
    fn a_lone_slash_is_ignored_like_an_empty_entry() {
        let filter = filter("/", "//");
        assert!(admits(&filter, "a.md"));
    }

    #[test]
    fn a_path_outside_the_root_is_refused_when_a_glob_is_set() {
        let restricted = filter("*.md", "");
        assert!(!restricted.admits(Path::new(ROOT), Path::new("/elsewhere/a.md")));
        let open = filter("", "");
        assert!(open.admits(Path::new(ROOT), Path::new("/elsewhere/a.md")));
    }

    #[test]
    fn an_unclosed_class_names_the_offending_field() {
        let include = fields("[", "").path_filter().expect_err("invalid include");
        assert!(include.0.starts_with("include: "), "{include:?}");
        let exclude = fields("*.md", "a/[")
            .path_filter()
            .expect_err("invalid exclude");
        assert!(exclude.0.starts_with("exclude: "), "{exclude:?}");
    }

    #[test]
    fn unicode_and_dot_paths_match_like_any_other() {
        let filter = filter("*.md", ".git/**");
        assert!(admits(&filter, "заметки/日本語.md"));
        assert!(!admits(&filter, ".git/notes.md"));
    }
}
