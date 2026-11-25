//! Sample data for the shared List, Table, and Tree components.

use marsa_ui::{
    Locale,
    components::{
        list::{ListDelegate, ListItem, ListState},
        table::{Column, TableDelegate, TableState},
        tree::{TreeItem, TreeState, tree},
    },
    direction::isolate_technical_text,
    gpui::{
        App, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
        div,
    },
    gpui_component::IndexPath,
    gui_keys as gui,
};

pub(super) struct Rows {
    count: usize,
    locale: Locale,
    filter: Option<Vec<usize>>,
}

impl Rows {
    pub(super) fn new(count: usize, locale: Locale) -> Self {
        Self {
            count,
            locale,
            filter: None,
        }
    }

    pub(super) fn set_filter(&mut self, query: &str) {
        let query = query.trim().to_ascii_lowercase();
        self.filter = (!query.is_empty()).then(|| {
            (0..self.count)
                .filter(|&row| Self::name(row).contains(&query))
                .collect()
        });
    }

    fn name(row: usize) -> SharedString {
        format!("sample-{:04}", row + 1).into()
    }

    fn cell(&self, row: usize, column: usize) -> SharedString {
        match column {
            0 => Self::name(row),
            _ => gui::GUI_GALLERY_READY.text(self.locale).into(),
        }
    }
}

impl ListDelegate for Rows {
    type Item = ListItem;

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.filter.as_ref().map_or(self.count, Vec::len)
    }

    fn set_selected_index(
        &mut self,
        _: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        // The list retains its own selection. There is no detail pane to update.
    }

    fn render_item(
        &mut self,
        index: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let row = match &self.filter {
            Some(filter) => *filter.get(index.row)?,
            None => index.row,
        };
        let name = Self::name(row);
        Some(
            ListItem::new(("example-list-row", index.row))
                .accessibility_label(name.clone())
                .child(isolate_technical_text(name.as_ref())),
        )
    }
}

impl TableDelegate for Rows {
    fn columns_count(&self, _: &App) -> usize {
        2
    }

    fn rows_count(&self, _: &App) -> usize {
        self.count
    }

    fn column(&self, index: usize, _: &App) -> Column {
        match index {
            0 => Column::new("name", gui::GUI_GALLERY_NAME.text(self.locale)),
            _ => Column::new("state", gui::GUI_GALLERY_STATE.text(self.locale)),
        }
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let value = self.cell(row, column);
        div().w_full().child(if column == 0 {
            isolate_technical_text(value.as_ref())
        } else {
            value
        })
    }

    fn cell_label(&self, row: usize, column: usize, _: &App) -> Option<SharedString> {
        Some(self.cell(row, column))
    }
}

pub(super) fn example_tree(state: &Entity<TreeState>) -> impl IntoElement {
    tree(state, |index, entry, _, _, _| {
        let item = entry.item();
        ListItem::new(("example-tree-row", index))
            .accessibility_label(item.label.clone())
            .child(item.label.clone())
    })
}

pub(super) fn tree_items(locale: Locale) -> Vec<TreeItem> {
    vec![
        TreeItem::new("workspace", gui::GUI_GALLERY_WORKSPACE.text(locale))
            .child(TreeItem::new("worker", "sample-0001"))
            .child(TreeItem::new("builder", "sample-0002"))
            .expanded(true),
        TreeItem::new("images", gui::GUI_GALLERY_IMAGES.text(locale))
            .child(TreeItem::new("image", "sample:latest")),
    ]
}
