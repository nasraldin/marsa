//! Search navigation and engine actions through the same command registry.

use marsa_client::api::EngineState;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        Context, Div, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
        Subscription, Window, div, px,
    },
    gpui_component::{
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
    },
};

use crate::{
    screens::common::{align, color, row},
    services::EngineAction,
};

use super::{Desktop, navigation::Route};

#[derive(Clone, Copy)]
enum Command {
    Navigate(Route),
    Engine(EngineAction),
}

impl Command {
    fn name(self, locale: Locale) -> &'static str {
        match self {
            Self::Navigate(route) => route.name(locale),
            Self::Engine(EngineAction::Start) => gui::GUI_DESKTOP_START_MACHINE.text(locale),
            Self::Engine(EngineAction::Stop) => gui::GUI_DESKTOP_STOP_MACHINE.text(locale),
            Self::Engine(EngineAction::Restart) => gui::GUI_DESKTOP_RESTART_MACHINE.text(locale),
        }
    }

    fn run(self, desktop: &mut Desktop, window: &mut Window, cx: &mut Context<Desktop>) {
        match self {
            Self::Navigate(route) => desktop.navigate(route, cx),
            Self::Engine(action) => desktop.request_engine(action, window, cx),
        }
    }
}

pub(super) fn observe_query(
    query: &marsa_ui::gpui::Entity<InputState>,
    window: &Window,
    cx: &mut Context<Desktop>,
) -> Subscription {
    cx.subscribe_in(query, window, |this, _, event, window, cx| match event {
        InputEvent::Change => cx.notify(),
        InputEvent::PressEnter { .. } if this.palette_open => {
            if let Some(command) = this.filtered_commands(cx).first().copied() {
                command.run(this, window, cx);
            }
        }
        _ => {}
    })
}

impl Desktop {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        let locale = self.locale;
        self.query.update(cx, |input, cx| {
            input.set_placeholder(gui::GUI_DESKTOP_SEARCH_COMMANDS.text(locale), window, cx);
            input.set_value("", window, cx);
        });
        let query = self.query.clone();
        window.on_next_frame(move |window, cx| {
            query.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.notify();
    }

    fn filtered_commands(&self, cx: &Context<Self>) -> Vec<Command> {
        let query = self.query.read(cx).value().trim().to_lowercase();
        let mut commands = Route::WORKSPACE
            .into_iter()
            .chain(Route::SYSTEM)
            .map(Command::Navigate)
            .collect::<Vec<_>>();
        match self.engine() {
            Some(EngineState::Stopped | EngineState::Failed) => {
                commands.push(Command::Engine(EngineAction::Start))
            }
            Some(EngineState::Ready | EngineState::Idle | EngineState::Paused) => {
                commands.extend([
                    Command::Engine(EngineAction::Stop),
                    Command::Engine(EngineAction::Restart),
                ]);
            }
            _ => {}
        }
        commands.retain(|command| {
            [Locale::EnglishUs, Locale::Arabic]
                .into_iter()
                .any(|locale| command.name(locale).to_lowercase().contains(&query))
        });
        commands
    }

    pub(super) fn command_palette(&self, cx: &mut Context<Self>) -> Div {
        let palette = self.palette;
        let locale = self.locale;
        let commands = self.filtered_commands(cx);
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(76.))
            .bg(color(palette, ColorToken::OverlayScrim).opacity(0.38))
            .child(
                div()
                    .w(px(560.))
                    .max_w_full()
                    .p_3()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(color(palette, ColorToken::BorderSubtle))
                    .bg(color(palette, ColorToken::BgElevated))
                    .shadow(vec![marsa_ui::tokens::DIALOG_ELEVATION.shadow()])
                    .flex()
                    .flex_col()
                    .gap_2()
                    .tab_group()
                    .child(
                        row(locale)
                            .gap_2()
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.query)
                                        .aria_label(gui::GUI_DESKTOP_SEARCH_COMMANDS.text(locale)),
                                ),
                            )
                            .child(
                                Button::new("close-search")
                                    .ghost()
                                    .label(gui::GUI_DESKTOP_CLOSE.text(locale))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.palette_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .children(commands.is_empty().then(|| {
                        align(div().p_3(), locale)
                            .child(gui::GUI_DESKTOP_NO_MATCHING_COMMANDS.text(locale))
                    }))
                    .child(
                        div()
                            .id("palette-results")
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(commands.into_iter().enumerate().map(|(index, command)| {
                                Button::new(("palette-command", index))
                                    .ghost()
                                    .w_full()
                                    .child(
                                        align(div().w_full(), locale).child(command.name(locale)),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        command.run(this, window, cx)
                                    }))
                            })),
                    ),
            )
    }
}
