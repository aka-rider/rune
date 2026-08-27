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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_row_add_and_sub_are_not_the_default() {
        assert_eq!(DisplayRow(5) + 3, DisplayRow(8));
        assert_eq!(DisplayRow(5) - 3, DisplayRow(2));
    }

    #[test]
    fn display_row_sub_saturates_instead_of_underflowing() {
        assert_eq!(DisplayRow(1) - 3, DisplayRow(0));
    }

    #[test]
    fn display_row_display_renders_the_number() {
        assert_eq!(DisplayRow(7).to_string(), "7");
    }
}
