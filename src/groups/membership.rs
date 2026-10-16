//! Which of the library's entries are in each group, worked out from the members' paths.

use std::collections::HashMap;

use super::{FAVORITES, GroupKind, Groups, UNCATEGORIZED};
use crate::asset::KindSet;
use crate::library::{Library, Located};

/// Where each group's members are in the open library. Rebuilt whenever the library or a group
/// changes; cheap, since each member costs one binary search.
#[derive(Default)]
pub struct Membership {
    /// Per group: its members in the library, as entry indices, in the group's order.
    pub entries: HashMap<i64, Vec<u32>>,
    /// Per entry that's in any group: those groups, in `Groups::all` order.
    of_entry: HashMap<u32, Vec<i64>>,
    /// Per group: how many members of each kind are in the library.
    totals: HashMap<i64, [u32; 3]>,
    /// Per group: members under the library's folder that aren't there any more (moved,
    /// renamed or deleted), by key.
    pub missing: HashMap<i64, Vec<String>>,
    /// Per group: how many members are outside the library's folder.
    pub elsewhere: HashMap<i64, usize>,
}

impl Membership {
    pub fn build(library: &Library, groups: &Groups) -> Self {
        let mut membership = Membership::default();
        let mut categorized = vec![false; library.entries().len()];
        for group in groups.all() {
            let mut entries = Vec::new();
            let mut totals = [0u32; 3];
            for member in &group.members {
                match library.locate(&member.key) {
                    Located::Found(index) => {
                        entries.push(index);
                        totals[library.entry(index).kind.index()] += 1;
                        membership.of_entry.entry(index).or_default().push(group.id);
                        if group.kind == GroupKind::Category {
                            categorized[index as usize] = true;
                        }
                    }
                    Located::Missing => membership.missing.entry(group.id).or_default().push(member.key.clone()),
                    Located::Outside => *membership.elsewhere.entry(group.id).or_default() += 1,
                }
            }
            membership.entries.insert(group.id, entries);
            membership.totals.insert(group.id, totals);
        }
        // Uncategorized: what no category holds, in library order.
        let mut totals = [0u32; 3];
        let uncategorized: Vec<u32> = (0..library.entries().len() as u32)
            .filter(|&index| !categorized[index as usize])
            .inspect(|&index| totals[library.entry(index).kind.index()] += 1)
            .collect();
        membership.entries.insert(UNCATEGORIZED, uncategorized);
        membership.totals.insert(UNCATEGORIZED, totals);
        membership
    }

    /// The groups the entry is in.
    pub fn groups_of(&self, index: u32) -> &[i64] {
        self.of_entry.get(&index).map_or(&[], Vec::as_slice)
    }

    pub fn is_favorite(&self, index: u32) -> bool {
        self.groups_of(index).contains(&FAVORITES)
    }

    pub fn totals(&self, id: i64) -> [u32; 3] {
        self.totals.get(&id).copied().unwrap_or_default()
    }

    /// Members of the enabled kinds in the library.
    pub fn total(&self, id: i64, kinds: KindSet) -> u32 {
        kinds.count(self.totals(id))
    }

    /// Whether `word` (lowercase) is in the name of a category the entry is in: how the search
    /// box finds assets by category.
    pub fn category_matches(&self, groups: &Groups, index: u32, word: &str) -> bool {
        self.groups_of(index).iter().any(|&id| {
            groups.get(id).is_some_and(|group| group.kind == GroupKind::Category && group.lower.contains(word))
        })
    }
}
