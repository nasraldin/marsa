//! Shared table chrome: filter/sort/columns dropdowns, headers, and row selection.

mod column_headers;
mod columns;
mod dropdown;
mod header;
mod row;
mod shell;
mod usage;
mod view_rail;

pub(crate) use column_headers::{default_flex, sortable_header};
pub(crate) use columns::{ColumnId, ColumnsPickerState, columns_picker};
pub(crate) use dropdown::{
    FilterMenuItem, SortField, SortMenuState, SortOrder, filter_dropdown, sort_dropdown,
};
pub(crate) use header::{SelectionToolbar, idle_toolbar, selection_toolbar};
pub(crate) use row::{cell, status_pill, table_row, tag_badge};
pub(crate) use shell::table_shell;
pub(crate) use usage::{UsageBucket, UsageFilter, preferred_in_use_tag};
pub(crate) use view_rail::{TableViewMode, view_mode_rail};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_click_toggles_order_on_same_field() {
        let state = SortMenuState::default();
        assert_eq!(state.field, SortField::Age);
        assert_eq!(state.order, SortOrder::Descending);
        let toggled = state.click_header(SortField::Age);
        assert_eq!(toggled.order, SortOrder::Ascending);
        let switched = toggled.click_header(SortField::Name);
        assert_eq!(switched.field, SortField::Name);
        assert_eq!(switched.order, SortOrder::Ascending);
        let size = switched.click_header(SortField::Size);
        assert_eq!(size.order, SortOrder::Descending);
    }

    #[test]
    fn sort_caption_uses_field_and_short_order() {
        let state = SortMenuState {
            field: SortField::Tags,
            order: SortOrder::Ascending,
        };
        assert_eq!(
            state.caption(marsa_ui::Locale::EnglishUs).as_ref(),
            "Tags (Asc)"
        );
        assert_eq!(
            SortMenuState {
                field: SortField::Size,
                order: SortOrder::Descending,
            }
            .caption(marsa_ui::Locale::EnglishUs)
            .as_ref(),
            "Size (Des)"
        );
    }

    #[test]
    fn columns_picker_pins_image_and_resets() {
        let mut state = ColumnsPickerState::default();
        assert_eq!(state.visible_ids().len(), 6);
        state.toggle(ColumnId::Tags);
        assert!(!state.visible_ids().contains(&ColumnId::Tags));
        state.toggle(ColumnId::Image);
        assert!(state.visible_ids().contains(&ColumnId::Image));
        state.move_column(ColumnId::Size, true);
        let order = state
            .columns
            .iter()
            .map(|column| column.id)
            .collect::<Vec<_>>();
        assert_eq!(order[0], ColumnId::Image);
        assert_eq!(ColumnsPickerState::reset().visible_ids().len(), 6);
    }

    #[test]
    fn columns_reorder_skips_pinned_image() {
        let mut state = ColumnsPickerState::default();
        state.move_column(ColumnId::Image, false);
        assert_eq!(state.columns[0].id, ColumnId::Image);
        state.move_column(ColumnId::Tags, false);
        assert_eq!(state.columns[1].id, ColumnId::Size);
        assert_eq!(state.columns[2].id, ColumnId::Tags);
        state.move_column(ColumnId::Tags, true);
        assert_eq!(state.columns[1].id, ColumnId::Tags);
    }
}
