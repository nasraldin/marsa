//! Sortable column header row.

use marsa_ui::{
    ColorToken,
    gpui::{
        App, Div, InteractiveElement as _, IntoElement, ParentElement as _,
        StatefulInteractiveElement as _, Styled as _, div, px, svg,
    },
    tokens::scale::Space,
};

use crate::screens::common::{ScreenContext, color, row};

use super::{ColumnId, SortMenuState, SortOrder};

/// One header cell: title plus caret when this field is the active sort.
pub(crate) fn sortable_header(
    screen: &ScreenContext<'_>,
    column: ColumnId,
    sort: SortMenuState,
    flex: f32,
    on_sort: impl Fn(SortMenuState, &mut App) + 'static,
) -> impl IntoElement {
    let label = column.label(screen.locale);
    let active = column.sort_field() == Some(sort.field);
    let sortable = column.sortable();
    let caret = active.then(|| sort_caret(screen, sort.order));
    let header = row(screen.locale)
        .id(("column-header", column as usize))
        .items_center()
        .gap_1()
        .h(px(Space::S36.pixels()))
        .px(px(Space::S8.pixels()))
        .min_w_0()
        .overflow_hidden()
        .flex_basis(px(0.))
        .flex_grow(flex)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_weight(marsa_ui::gpui::FontWeight::MEDIUM)
                .text_color(color(
                    screen.palette,
                    if active {
                        ColorToken::TextPrimary
                    } else {
                        ColorToken::TextMuted
                    },
                ))
                .child(label),
        )
        .children(caret);
    if !sortable {
        return header.into_any_element();
    }
    let field = column
        .sort_field()
        .expect("sortable columns expose a sort field");
    header
        .cursor_pointer()
        .on_click(move |_, _, cx| on_sort(sort.click_header(field), cx))
        .into_any_element()
}

fn sort_caret(screen: &ScreenContext<'_>, order: SortOrder) -> Div {
    let path = match order {
        SortOrder::Ascending => "icons/chevron-up.svg",
        SortOrder::Descending => "icons/chevron-down.svg",
    };
    div().flex_shrink_0().child(
        svg()
            .path(path)
            .size(px(12.))
            .text_color(color(screen.palette, ColorToken::TextMuted)),
    )
}

/// Default flex weights for the Images column set.
///
/// Compact columns (Size, Age, Actions) stay narrow so Image/Tags can grow.
pub(crate) fn default_flex(column: ColumnId) -> f32 {
    match column {
        ColumnId::Image => 260.,
        ColumnId::Tags => 140.,
        ColumnId::Size => 72.,
        ColumnId::Age => 52.,
        ColumnId::State => 100.,
        ColumnId::Actions => 112.,
    }
}
