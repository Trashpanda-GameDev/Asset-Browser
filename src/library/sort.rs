//! The orders the browsed assets can be put in.

use super::Library;

/// An order for the assets: what they're sorted by, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub by: SortBy,
    pub descending: bool,
}

impl Order {
    /// Sorted by `by`, the way that starts out (`SortBy::starts_descending`).
    pub fn starting(by: SortBy) -> Self {
        Self { by, descending: by.starts_descending() }
    }
}

/// The order of the browsed assets. `Folder` is the library's own order: by path, so each
/// folder's files stay together. `Added` is the order assets went into a collection or category
/// (see `groups`); in a folder it's the same as `Folder`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Added,
    Folder,
    Name,
    Type,
    Size,
    Modified,
}

impl SortBy {
    /// The orders a folder offers.
    pub const ALL: [SortBy; 5] = [Self::Folder, Self::Name, Self::Type, Self::Size, Self::Modified];
    /// The orders a collection or category offers.
    pub const IN_GROUPS: [SortBy; 6] = [Self::Added, Self::Folder, Self::Name, Self::Type, Self::Size, Self::Modified];

    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Folder => "Folder",
            Self::Name => "Name",
            Self::Type => "Type",
            Self::Size => "Size",
            Self::Modified => "Modified",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Folder => "folder",
            Self::Name => "name",
            Self::Type => "type",
            Self::Size => "size",
            Self::Modified => "modified",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::IN_GROUPS.into_iter().find(|sort| sort.key() == key)
    }

    /// Biggest, newest and most recently added first is what people look for.
    pub fn starts_descending(self) -> bool {
        matches!(self, Self::Size | Self::Modified | Self::Added)
    }
}

impl Library {
    /// Puts `view` (indices into `entries`, in library order) in the given order. Ties keep the
    /// library order, whichever the direction.
    pub fn sort(&self, view: &mut [u32], by: SortBy, descending: bool) {
        let entry = |index: &u32| &self.entries[*index as usize];
        view.sort_unstable_by(|a, b| {
            let (x, y) = (entry(a), entry(b));
            let order = match by {
                SortBy::Folder | SortBy::Added => std::cmp::Ordering::Equal,
                SortBy::Name => x.lower_name().cmp(y.lower_name()),
                SortBy::Type => {
                    x.lower_extension().cmp(y.lower_extension()).then_with(|| x.lower_name().cmp(y.lower_name()))
                }
                SortBy::Size => x.size.cmp(&y.size),
                SortBy::Modified => x.mtime.cmp(&y.mtime),
            };
            let order = if descending { order.reverse() } else { order };
            // Library order breaks ties (and is the whole order for `Folder`).
            let whole = matches!(by, SortBy::Folder | SortBy::Added);
            order.then(if whole && descending { b.cmp(a) } else { a.cmp(b) })
        });
    }
}
