//! Representative Kit controls with inert counters instead of external actions.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Window, actions,
    base::LogicalStyledExt as _,
    component::{
        ActiveTheme as _, Disableable as _, WindowExt as _,
        button::Button,
        dialog::DialogEntrance,
        input::{Input, InputState},
        menu::DropdownMenu as _,
    },
    div,
};

actions!(
    controls,
    [
        /// Records one successful Inspect command in the intentionally inert fixture.
        RunMenuCommand,
        /// Records an erroneous activation of a disabled command in the fixture.
        DisabledMenuCommand,
    ]
);

#[derive(Clone, Copy)]
pub(super) struct Labels {
    pub(super) start: &'static str,
    pub(super) remove: &'static str,
    pub(super) menu: &'static str,
    pub(super) disabled: &'static str,
    pub(super) menu_run: &'static str,
    pub(super) dialog: &'static str,
    pub(super) input: &'static str,
    pub(super) confirm: &'static str,
    pub(super) cancel: &'static str,
}

pub(super) const ENGLISH: Labels = Labels {
    start: "Start container",
    remove: "Remove container",
    menu: "Container actions",
    disabled: "Unavailable",
    menu_run: "Inspect container",
    dialog: "Confirm removal",
    input: "Type REMOVE to confirm",
    confirm: "Confirm",
    cancel: "Cancel",
};

pub(super) const ARABIC: Labels = Labels {
    start: "شغّل الحاوية",
    remove: "أزل الحاوية",
    menu: "إجراءات الحاوية",
    disabled: "غير متاح",
    menu_run: "افحص الحاوية",
    dialog: "تأكيد الإزالة",
    input: "اكتب \u{2066}REMOVE\u{2069} للتأكيد",
    confirm: "تأكيد",
    cancel: "إلغاء",
};

#[derive(Default)]
pub(super) struct Commands {
    pub(super) started: Cell<u32>,
    pub(super) removed: Cell<u32>,
    pub(super) inspected: Cell<u32>,
    pub(super) disabled: Cell<u32>,
}

pub(super) struct Controls {
    pub(super) labels: Labels,
    pub(super) commands: Rc<Commands>,
    pub(super) initial: FocusHandle,
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let labels = self.labels;
        let start = self.commands.clone();
        let remove = self.commands.clone();
        let disabled = self.commands.clone();
        let inspect = self.commands.clone();
        let blocked_menu = self.commands.clone();

        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .flex()
            .flex_row_dir(cx)
            .gap_3()
            .p_4()
            .items_start()
            .on_action(move |_: &RunMenuCommand, _, _| {
                inspect.inspected.set(inspect.inspected.get() + 1);
            })
            .on_action(move |_: &DisabledMenuCommand, _, _| {
                blocked_menu.disabled.set(blocked_menu.disabled.get() + 1);
            })
            .child(
                button("start", labels.start)
                    .track_focus(&self.initial)
                    .on_click(move |_, _, _| {
                        start.started.set(start.started.get() + 1);
                    }),
            )
            .child(
                button("disabled", labels.disabled)
                    .disabled(true)
                    .on_click(move |_, _, _| disabled.disabled.set(disabled.disabled.get() + 1)),
            )
            .child(
                button("remove", labels.remove).on_click(move |_, window, cx| {
                    open_confirmation(labels, remove.clone(), window, cx);
                }),
            )
            .child(
                button("menu", labels.menu).dropdown_menu(move |menu, _, _| {
                    menu.menu_with_disabled(labels.disabled, Box::new(DisabledMenuCommand), true)
                        .menu(labels.menu_run, Box::new(RunMenuCommand))
                }),
            )
    }
}

fn button(id: &'static str, label: &'static str) -> Button {
    Button::new(id)
        .label(label)
        .accessibility_label(label)
        .tab_stop(true)
}

fn guarded_remove(input: &Entity<InputState>, commands: &Commands, cx: &App) -> bool {
    if input.read(cx).value().as_ref() != "REMOVE" {
        return false;
    }
    commands.removed.set(commands.removed.get() + 1);
    true
}

fn open_confirmation(labels: Labels, commands: Rc<Commands>, window: &mut Window, cx: &mut App) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(labels.input));
    window.open_dialog(cx, move |dialog, _, cx| {
        let guarded_input = input.clone();
        let guarded_commands = commands.clone();
        let click_input = input.clone();
        let click_commands = commands.clone();
        dialog
            .title(labels.dialog)
            .entrance(DialogEntrance::None)
            .close_button(false)
            .overlay_closable(false)
            .child(Input::new(&input).aria_label(labels.input))
            .on_ok(move |_, _, cx| guarded_remove(&guarded_input, &guarded_commands, cx))
            .footer(
                div()
                    .flex()
                    .flex_row_dir(cx)
                    .gap_3()
                    .child(button("cancel", labels.cancel).on_click(|_, window, cx| {
                        window.close_dialog(cx);
                    }))
                    .child(
                        button("confirm", labels.confirm).on_click(move |_, window, cx| {
                            if guarded_remove(&click_input, &click_commands, cx) {
                                window.close_dialog(cx);
                            }
                        }),
                    ),
            )
    });
}
