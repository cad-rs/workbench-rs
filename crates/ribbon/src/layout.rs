//! Group column layout (docs/ribbon-bar.md section 2.3).
//!
//! A group renders as a list of columns, declared order left to right:
//! - `Large` items occupy a full 48px column on their own;
//! - `Small` / `Icon` / `Combo` items stack vertically, up to
//!   [`MAX_STACK_ROWS`] rows per column, then a new column starts.
//!
//! This module is pure logic (no gpui) so the row assignment stays unit
//! testable.

use crate::model::{ItemSize, RibbonItemVm};
use crate::tokens::MAX_STACK_ROWS;

/// The planned column sequence of one group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnPlan {
    /// A single Large item filling its own column; payload is the item index.
    Large(usize),
    /// A stacked column of 1..=MAX_STACK_ROWS rows; payload are item indices
    /// in declaration order (top to bottom).
    Stack(Vec<usize>),
}

/// True when the item renders as a full-width Large column.
pub fn is_large(item: &RibbonItemVm) -> bool {
    item.size == ItemSize::Large
}

/// Assigns items of a group to columns per section 2.3.
pub fn plan_columns(items: &[RibbonItemVm]) -> Vec<ColumnPlan> {
    let mut plans: Vec<ColumnPlan> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for (ix, item) in items.iter().enumerate() {
        if is_large(item) {
            if !stack.is_empty() {
                plans.push(ColumnPlan::Stack(std::mem::take(&mut stack)));
            }
            plans.push(ColumnPlan::Large(ix));
        } else {
            stack.push(ix);
            if stack.len() == MAX_STACK_ROWS {
                plans.push(ColumnPlan::Stack(std::mem::take(&mut stack)));
            }
        }
    }

    if !stack.is_empty() {
        plans.push(ColumnPlan::Stack(stack));
    }
    plans
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
    fn declaration_order_is_column_order() {
        // [Large A][Small B / Icon C][Large D][Combo E] per section 2.3.
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
                ColumnPlan::Stack(vec![1, 2]),
                ColumnPlan::Large(3),
                ColumnPlan::Stack(vec![4]),
            ]
        );
    }

    #[test]
    fn empty_group_has_no_columns() {
        assert!(plan_columns(&[]).is_empty());
    }
}
