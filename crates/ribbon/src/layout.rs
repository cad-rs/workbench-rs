//! Group column layout (docs/ribbon-bar.md section 2.3).
//!
//! A group renders as a list of columns, declared order left to right:
//! - `Large` items occupy a full 48px column on their own;
//! - `Small` / `Combo` items stack vertically, up to [`MAX_STACK_ROWS`] rows
//!   per column, then a new column starts;
//! - runs of consecutive `Icon` items pack horizontally, up to
//!   [`ICONS_PER_ROW`] buttons per row (one row == one 28px slot), then the
//!   next icon row starts.
//!
//! This module is pure logic (no gpui) so the row assignment stays unit
//! testable.

use crate::model::{ItemSize, RibbonGroupVm, RibbonItemVm, RibbonTabVm};
use crate::tokens::{ICONS_PER_ROW, MAX_STACK_ROWS};

/// The planned column sequence of one group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnPlan {
    /// A single Large item filling its own column; payload is the item index.
    Large(usize),
    /// A stacked column of 1..=MAX_STACK_ROWS rows; payload are item indices
    /// in declaration order (top to bottom).
    Stack(Vec<usize>),
    /// One row of horizontally packed icon-tier buttons; payload are item
    /// indices in declaration order (left to right).
    IconRow(Vec<usize>),
}

/// True when the item renders as a full-width Large column.
pub fn is_large(item: &RibbonItemVm) -> bool {
    item.size == ItemSize::Large
}

/// True when the item packs horizontally in an icon row.
fn is_icon(item: &RibbonItemVm) -> bool {
    item.size == ItemSize::Icon
}

/// Assigns items of a group to columns per section 2.3.
pub fn plan_columns(items: &[RibbonItemVm]) -> Vec<ColumnPlan> {
    let mut plans: Vec<ColumnPlan> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut icon_row: Vec<usize> = Vec::new();

    // Flushes whichever run is open, preferring the most recent one.
    macro_rules! flush_open {
        () => {
            if !icon_row.is_empty() {
                plans.push(ColumnPlan::IconRow(std::mem::take(&mut icon_row)));
            }
            if !stack.is_empty() {
                plans.push(ColumnPlan::Stack(std::mem::take(&mut stack)));
            }
        };
    }

    for (ix, item) in items.iter().enumerate() {
        if is_large(item) {
            flush_open!();
            plans.push(ColumnPlan::Large(ix));
        } else if is_icon(item) {
            // An open Small stack ends when icons start; the icon run may
            // share the column with prior Small rows only if it started
            // right after them. Keep it simple and spec-true: icon runs
            // always own their row slots.
            if !stack.is_empty() {
                plans.push(ColumnPlan::Stack(std::mem::take(&mut stack)));
            }
            icon_row.push(ix);
            if icon_row.len() == ICONS_PER_ROW {
                plans.push(ColumnPlan::IconRow(std::mem::take(&mut icon_row)));
            }
        } else {
            if !icon_row.is_empty() {
                plans.push(ColumnPlan::IconRow(std::mem::take(&mut icon_row)));
            }
            stack.push(ix);
            if stack.len() == MAX_STACK_ROWS {
                plans.push(ColumnPlan::Stack(std::mem::take(&mut stack)));
            }
        }
    }

    if !icon_row.is_empty() {
        plans.push(ColumnPlan::IconRow(icon_row));
    }
    if !stack.is_empty() {
        plans.push(ColumnPlan::Stack(stack));
    }
    plans
}

/// How many groups, counting from the right, carry at least one Large item
/// and can therefore be degraded one step.
pub fn count_degradable_groups(groups: &[RibbonGroupVm]) -> usize {
    groups
        .iter()
        .rev()
        .filter(|g| g.items.iter().any(|it| it.size == ItemSize::Large))
        .count()
}

/// A copy of the active tab with the right-most `level` Large-carrying
/// groups degraded: every Large item in those groups becomes Small, so the
/// group compresses into stacked columns. `level` is clamped by
/// [`count_degradable_groups`] semantics (extra level is a no-op).
pub fn degrade_tab(tab: Option<&RibbonTabVm>, level: usize) -> Option<RibbonTabVm> {
    let mut tab = tab?.clone();
    let mut remaining = level;
    for group in tab.groups.iter_mut().rev() {
        if remaining == 0 {
            break;
        }
        if group.items.iter().any(|it| it.size == ItemSize::Large) {
            for it in group.items.iter_mut() {
                if it.size == ItemSize::Large {
                    it.size = ItemSize::Small;
                }
            }
            remaining -= 1;
        }
    }
    Some(tab)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RibbonItemKindVm, RibbonItemVm};

    fn item(id: &str, size: ItemSize) -> RibbonItemVm {
        RibbonItemVm {
            id: id.to_string(),
            label: id.to_string(),
            icon: None,
            enabled: true,
            size,
            width: None,
            kind: RibbonItemKindVm::Command,
        }
    }

    #[test]
    fn large_items_each_own_a_column() {
        let items = vec![item("a", ItemSize::Large), item("b", ItemSize::Large)];
        assert_eq!(
            plan_columns(&items),
            vec![ColumnPlan::Large(0), ColumnPlan::Large(1)]
        );
    }

    #[test]
    fn small_items_stack_and_break_after_three_rows() {
        let items: Vec<_> = (0..4)
            .map(|ix| item(&format!("s{ix}"), ItemSize::Small))
            .collect();
        assert_eq!(
            plan_columns(&items),
            vec![
                ColumnPlan::Stack(vec![0, 1, 2]),
                ColumnPlan::Stack(vec![3]),
            ]
        );
    }

    #[test]
    fn icon_items_pack_four_per_row() {
        let items: Vec<_> = (0..5)
            .map(|ix| item(&format!("i{ix}"), ItemSize::Icon))
            .collect();
        assert_eq!(
            plan_columns(&items),
            vec![
                ColumnPlan::IconRow(vec![0, 1, 2, 3]),
                ColumnPlan::IconRow(vec![4]),
            ]
        );
    }

    #[test]
    fn small_run_breaks_when_icons_start() {
        let items = vec![
            item("s0", ItemSize::Small),
            item("s1", ItemSize::Small),
            item("i0", ItemSize::Icon),
            item("i1", ItemSize::Icon),
        ];
        assert_eq!(
            plan_columns(&items),
            vec![
                ColumnPlan::Stack(vec![0, 1]),
                ColumnPlan::IconRow(vec![2, 3]),
            ]
        );
    }

    #[test]
    fn declaration_order_is_column_order() {
        // [Large A][Small B / Icon C][Large D][Combo E] per section 2.3.
        // A lone Icon in the Small run still opens its own icon row.
        let items = vec![
            item("a", ItemSize::Large),
            item("b", ItemSize::Small),
            item("c", ItemSize::Icon),
            item("d", ItemSize::Large),
            item("e", ItemSize::Small),
        ];
        assert_eq!(
            plan_columns(&items),
            vec![
                ColumnPlan::Large(0),
                ColumnPlan::Stack(vec![1]),
                ColumnPlan::IconRow(vec![2]),
                ColumnPlan::Large(3),
                ColumnPlan::Stack(vec![4]),
            ]
        );
    }

    #[test]
    fn empty_group_has_no_columns() {
        assert!(plan_columns(&[]).is_empty());
    }

    fn group(id: &str, sizes: &[ItemSize]) -> crate::model::RibbonGroupVm {
        crate::model::RibbonGroupVm {
            id: id.to_string(),
            title: id.to_string(),
            items: sizes
                .iter()
                .enumerate()
                .map(|(ix, s)| item(&format!("{id}{ix}"), *s))
                .collect(),
        }
    }

    fn tab(groups: Vec<crate::model::RibbonGroupVm>) -> crate::model::RibbonTabVm {
        crate::model::RibbonTabVm {
            id: "t".into(),
            title: "t".into(),
            contextual: false,
            groups,
        }
    }

    #[test]
    fn degrade_counts_large_groups_from_the_right() {
        let t = tab(vec![
            group("g0", &[ItemSize::Large]),
            group("g1", &[ItemSize::Small]),
            group("g2", &[ItemSize::Large, ItemSize::Large]),
        ]);
        assert_eq!(count_degradable_groups(&t.groups), 2);
    }

    #[test]
    fn degrade_demotes_right_most_large_groups_first() {
        let t = tab(vec![
            group("g0", &[ItemSize::Large]),
            group("g1", &[ItemSize::Small]),
            group("g2", &[ItemSize::Large, ItemSize::Large]),
        ]);
        let degraded = degrade_tab(Some(&t), 1).expect("tab");
        // g2 (right-most) demoted, g0 untouched.
        assert_eq!(degraded.groups[2].items[0].size, ItemSize::Small);
        assert_eq!(degraded.groups[0].items[0].size, ItemSize::Large);

        let degraded = degrade_tab(Some(&t), 5).expect("tab");
        // Level above the degradable count demotes everything.
        assert_eq!(degraded.groups[0].items[0].size, ItemSize::Small);
        assert_eq!(degraded.groups[2].items[1].size, ItemSize::Small);
    }

    #[test]
    fn degrade_none_leaves_tab_untouched() {
        let t = tab(vec![group("g0", &[ItemSize::Large])]);
        let degraded = degrade_tab(Some(&t), 0).expect("tab");
        assert_eq!(degraded.groups[0].items[0].size, ItemSize::Large);
        assert!(degrade_tab(None, 1).is_none());
    }
}
