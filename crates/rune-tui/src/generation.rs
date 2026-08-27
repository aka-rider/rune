use std::marker::PhantomData;

// Phantom-typed over a zero-sized marker per feature, so a rename
// generation and a trash generation share the same `u64` shape but can
// never be compared or swapped for one another by mistake. `T` never
// appears at runtime, so every trait below is implemented for every `T`
// unconditionally rather than derived (`derive` would wrongly require `T:
// Trait`).
pub struct Generation<T>(u64, PhantomData<fn() -> T>);

impl<T> Generation<T> {
    pub const ZERO: Generation<T> = Generation(0, PhantomData);

    // For a fuzz driver deliberately targeting a generation `mint()`
    // structurally cannot reach. Production code only ever holds a
    // `Generation` a `GenCounter` minted.
    pub const fn from_raw(value: u64) -> Generation<T> {
        Generation(value, PhantomData)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl<T> Clone for Generation<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Generation<T> {}

impl<T> std::fmt::Debug for Generation<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Generation").field(&self.0).finish()
    }
}

impl<T> Default for Generation<T> {
    fn default() -> Self {
        Generation::ZERO
    }
}

impl<T> PartialEq for Generation<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T> Eq for Generation<T> {}

impl<T> std::hash::Hash for Generation<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

pub struct GenCounter<T>(Generation<T>);

impl<T> Clone for GenCounter<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for GenCounter<T> {}

impl<T> std::fmt::Debug for GenCounter<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("GenCounter").field(&self.0).finish()
    }
}

impl<T> Default for GenCounter<T> {
    fn default() -> Self {
        GenCounter(Generation::ZERO)
    }
}

impl<T> GenCounter<T> {
    pub fn mint(&mut self) -> Generation<T> {
        let minted = self.0;
        self.0 = Generation::from_raw(self.0.0.wrapping_add(1));
        minted
    }
}

pub struct Rename;
pub struct Merge;
pub struct SaveConfirm;
pub struct Quit;
pub struct Trash;
pub struct FileSearch;
pub struct SearchHistory;
pub struct Palette;
pub struct DirLoad;
pub struct ImageDecode;
pub struct MessagesCollapse;
pub struct Preview;

pub type RenameGen = Generation<Rename>;
pub type MergeGen = Generation<Merge>;
pub type SaveConfirmGen = Generation<SaveConfirm>;
pub type QuitGen = Generation<Quit>;
pub type TrashGen = Generation<Trash>;
pub type FileSearchGen = Generation<FileSearch>;
pub type SearchHistoryGen = Generation<SearchHistory>;
pub type PaletteGen = Generation<Palette>;
pub type DirLoadGen = Generation<DirLoad>;
pub type ImageDecodeGen = Generation<ImageDecode>;
pub type MessagesCollapseGen = Generation<MessagesCollapse>;
pub type PreviewGen = Generation<Preview>;
