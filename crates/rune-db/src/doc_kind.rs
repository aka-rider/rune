#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocKind {
    File,
    Scratch,
}

impl DocKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            DocKind::File => "file",
            DocKind::Scratch => "scratch",
        }
    }
}
