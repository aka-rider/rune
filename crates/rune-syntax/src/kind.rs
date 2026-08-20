use crate::lang::LangId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocumentKind {
    #[default]
    Markdown,
    Code(LangId),
    Plain,
    Image,
}

impl DocumentKind {
    pub fn is_markdown(&self) -> bool {
        matches!(self, DocumentKind::Markdown)
    }

    pub fn language(&self) -> Option<&'static str> {
        match self {
            DocumentKind::Code(lang) => Some(lang.name()),
            DocumentKind::Markdown | DocumentKind::Plain | DocumentKind::Image => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn rust() -> LangId {
        LangId::from_name("rust").unwrap()
    }

    #[test]
    fn default_is_markdown() {
        assert_eq!(DocumentKind::default(), DocumentKind::Markdown);
    }

    #[test]
    fn only_code_carries_a_language() {
        assert_eq!(DocumentKind::Markdown.language(), None);
        assert_eq!(DocumentKind::Plain.language(), None);
        assert_eq!(DocumentKind::Image.language(), None);
        assert_eq!(DocumentKind::Code(rust()).language(), Some("rust"));
    }

    #[test]
    fn is_markdown_is_true_only_for_markdown() {
        assert!(DocumentKind::Markdown.is_markdown());
        assert!(!DocumentKind::Code(rust()).is_markdown());
        assert!(!DocumentKind::Plain.is_markdown());
        assert!(!DocumentKind::Image.is_markdown());
    }
}
