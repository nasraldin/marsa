//! Pointer and keyboard controls for a horizontally divided workspace.

use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    accesskit::Role, div, px,
};
use gpui_component::resizable::{ResizableState, h_resizable, resizable_panel};

use crate::{ColorToken, Locale, Palette, tokens::scale::Space};

use super::color;

/// The focus and panel widths retained by a two-pane split view.
pub struct SplitView {
    focus: FocusHandle,
    widths: [Entity<ResizableState>; 2],
}

impl SplitView {
    /// Create equal panes and a focus stop for the divider.
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            widths: std::array::from_fn(|_| cx.new(|_| ResizableState::default())),
        }
    }

    /// Lay out the panes in reading order and move the divider with arrow keys.
    pub fn render(
        &self,
        id: impl Into<ElementId>,
        locale: Locale,
        palette: Palette,
        label: impl Into<SharedString>,
        panes: [AnyElement; 2],
    ) -> impl IntoElement {
        let rtl = locale == Locale::Arabic;
        let layout = self.widths[usize::from(rtl)].clone();
        let resize = layout.clone();
        let focus = self.focus.clone();
        let [start, end] = panes;
        let panes = if rtl { [end, start] } else { [start, end] };
        div()
            .id(id)
            .track_focus(&self.focus)
            .tab_stop(true)
            .role(Role::Splitter)
            .aria_label(label)
            .aria_keyshortcuts("Left Right")
            .size_full()
            .border_1()
            .border_color(color(palette, ColorToken::BorderSubtle))
            .focus_visible(move |style| style.border_color(color(palette, ColorToken::AccentFocus)))
            .on_key_down(move |event, window, cx| {
                if !focus.is_focused(window) {
                    return;
                }
                let delta = match event.keystroke.key.as_str() {
                    "left" => -Space::S16.pixels(),
                    "right" => Space::S16.pixels(),
                    _ => return,
                };
                resize.update(cx, |layout, cx| {
                    if let Some(&width) = layout.sizes().first() {
                        layout.resize_panel(0, width + px(delta), window, cx);
                    }
                });
                cx.stop_propagation();
            })
            .child(
                h_resizable("split-panels")
                    .with_state(&layout)
                    .children(panes.map(|pane| resizable_panel().child(pane))),
            )
    }
}
