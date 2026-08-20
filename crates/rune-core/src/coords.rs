#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferPoint {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VisualCol(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyntaxOffset(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyntaxPoint {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WrapRow(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WrapPoint {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayRow(pub usize);

impl std::ops::Add<usize> for DisplayRow {
    type Output = DisplayRow;

    fn add(self, rhs: usize) -> DisplayRow {
        DisplayRow(self.0.saturating_add(rhs))
    }
}

impl std::ops::Sub<usize> for DisplayRow {
    type Output = DisplayRow;

    fn sub(self, rhs: usize) -> DisplayRow {
        DisplayRow(self.0.saturating_sub(rhs))
    }
}

impl std::fmt::Display for DisplayRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayPoint {
    pub row: usize,
    pub col: usize,
}
