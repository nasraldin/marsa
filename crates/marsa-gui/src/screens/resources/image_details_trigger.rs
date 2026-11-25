//! Focusable entry point to an image's details.

use marsa_ui::{
    gpui::{self, App, IntoElement, RenderOnce, SharedString, Styled as _, Window, px},
    gpui_component::{
        Disableable as _, Icon,
        button::{Button, ButtonVariants as _},
    },
};

#[derive(IntoElement)]
pub(super) struct ImageDetailsTrigger<F: Fn(&mut Window, &mut App) + 'static> {
    key: SharedString,
    tooltip: &'static str,
    label: SharedString,
    enabled: bool,
    open: F,
}

impl<F: Fn(&mut Window, &mut App) + 'static> ImageDetailsTrigger<F> {
    pub(super) fn new(
        key: impl Into<SharedString>,
        tooltip: &'static str,
        label: SharedString,
        enabled: bool,
        open: F,
    ) -> Self {
        Self {
            key: key.into(),
            tooltip,
            label,
            enabled,
            open,
        }
    }
}

impl<F: Fn(&mut Window, &mut App) + 'static> RenderOnce for ImageDetailsTrigger<F> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(
                (
                    gpui::ElementId::from("image-details-focus"),
                    self.key.clone(),
                ),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        Button::new((gpui::ElementId::from("image-details"), self.key))
            .ghost()
            .size(px(28.))
            .p_0()
            .flex_shrink_0()
            .icon(Icon::default().path("icons/info.svg"))
            .tooltip(self.tooltip)
            .accessibility_label(self.label)
            .disabled(!self.enabled)
            .track_focus(&focus)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                // Component buttons preserve the previous focus on a mouse press.
                // Focus this trigger before the dialog saves its return target.
                focus.focus(window, cx);
                (self.open)(window, cx);
            })
    }
}
