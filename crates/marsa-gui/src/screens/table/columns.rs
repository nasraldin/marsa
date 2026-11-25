//! Columns visibility and order picker.

use marsa_ui::{
    Locale,
    gpui::{App, IntoElement, SharedString, Styled as _, px},
    gpui_component::{
        Icon,
        button::Button,
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
};

use crate::screens::common::{ScreenContext, message};

/// One configurable table column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub(crate) enum ColumnId {
    Image = 0,
    Tags = 1,
    Size = 2,
    Age = 3,
    State = 4,
}

impl ColumnId {
    pub(crate) const DEFAULT_ORDER: [Self; 5] =
        [Self::Image, Self::Tags, Self::Size, Self::Age, Self::State];

    pub(crate) fn label(self, locale: Locale) -> SharedString {
        message(
            locale,
            match self {
                Self::Image => gui::GUI_DESKTOP_IMAGE,
                Self::Tags => gui::GUI_DESKTOP_TAGS,
                Self::Size => gui::GUI_DESKTOP_SIZE,
                Self::Age => gui::GUI_DESKTOP_AGE,
                Self::State => gui::GUI_DESKTOP_STATE,
            },
        )
    }

    pub(crate) fn pinned(self) -> bool {
        matches!(self, Self::Image)
    }

    pub(crate) fn sortable(self) -> bool {
        true
    }

    pub(crate) fn sort_field(self) -> Option<super::SortField> {
        Some(match self {
            Self::Image => super::SortField::Name,
            Self::Tags => super::SortField::Tags,
            Self::Size => super::SortField::Size,
            Self::Age => super::SortField::Age,
            Self::State => super::SortField::State,
        })
    }
}

/// Spec for the columns picker list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ColumnSpec {
    pub id: ColumnId,
    pub visible: bool,
}

/// Ordered visible column configuration with reset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ColumnsPickerState {
    pub columns: Vec<ColumnSpec>,
}

impl Default for ColumnsPickerState {
    fn default() -> Self {
        Self {
            columns: ColumnId::DEFAULT_ORDER
                .into_iter()
                .map(|id| ColumnSpec { id, visible: true })
                .collect(),
        }
    }
}

impl ColumnsPickerState {
    pub(crate) fn reset() -> Self {
        Self::default()
    }

    pub(crate) fn visible_ids(&self) -> Vec<ColumnId> {
        self.columns
            .iter()
            .filter(|column| column.visible || column.id.pinned())
            .map(|column| column.id)
            .collect()
    }

    pub(crate) fn toggle(&mut self, id: ColumnId) {
        if id.pinned() {
            return;
        }
        if let Some(column) = self.columns.iter_mut().find(|column| column.id == id) {
            column.visible = !column.visible;
        }
    }

    /// Reorder a non-pinned column. Covered by unit tests; picker UI can call this later.
    #[allow(dead_code)]
    pub(crate) fn move_column(&mut self, id: ColumnId, up: bool) {
        let Some(index) = self.columns.iter().position(|column| column.id == id) else {
            return;
        };
        if self.columns[index].id.pinned() {
            return;
        }
        let swap = if up {
            (1..index)
                .rev()
                .find(|&candidate| !self.columns[candidate].id.pinned())
        } else {
            ((index + 1)..self.columns.len())
                .find(|&candidate| !self.columns[candidate].id.pinned())
        };
        if let Some(swap) = swap {
            self.columns.swap(index, swap);
        }
    }
}

/// Sliders trigger that opens the Columns popover.
pub(crate) fn columns_picker(
    id: &'static str,
    screen: &ScreenContext<'_>,
    state: ColumnsPickerState,
    on_change: impl Fn(ColumnsPickerState, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let locale = screen.locale;
    let title = message(locale, gui::GUI_DESKTOP_COLUMNS);
    Button::new(id)
        .outline()
        .size(px(36.))
        .p_0()
        .icon(Icon::default().path("icons/sliders-horizontal.svg"))
        .accessibility_label(title.clone())
        .tooltip(title)
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu
                .item(PopupMenuItem::label(message(
                    locale,
                    gui::GUI_DESKTOP_COLUMNS,
                )))
                .item(
                    PopupMenuItem::new(message(locale, gui::GUI_DESKTOP_RESET_COLUMNS))
                        .icon(Icon::default().path("icons/rotate-ccw.svg"))
                        .on_click({
                            let change = on_change.clone();
                            move |_, _, cx| change(ColumnsPickerState::reset(), cx)
                        }),
                )
                .separator();
            for column in &state.columns {
                let change = on_change.clone();
                let baseline = state.clone();
                let column_id = column.id;
                let pinned = column.id.pinned();
                let visible = column.visible || pinned;
                menu = menu.item(
                    PopupMenuItem::new(column.id.label(locale))
                        .checked(visible)
                        .disabled(pinned)
                        .on_click(move |_, _, cx| {
                            let mut next = baseline.clone();
                            next.toggle(column_id);
                            change(next, cx);
                        }),
                );
            }
            menu
        })
}
