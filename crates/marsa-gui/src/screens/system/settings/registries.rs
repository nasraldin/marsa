//! Manage desktop registry credentials and verify the registry over HTTPS.

use marsa_client::docker::{
    RegistryAddress, RegistryCredentials, RegistryEntry, RegistryError, RegistryStore,
};
use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        AppContext, Context, Div, Entity, FontWeight, ParentElement, PromptButton, PromptLevel,
        SharedString, Styled, Task, Window, div, px,
    },
    gpui_component::{
        Disableable, WindowExt,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
    },
    gui_keys as gui,
    tokens::typography::{Role, ui_size},
};

use crate::screens::common::{
    ScreenContext, color, dialog_actions, dialog_frame, dialog_heading, row, technical,
};

use super::{SystemView, settings_card, settings_heading, settings_text};

#[derive(Default)]
pub(in crate::screens::system) struct RegistrySettings {
    entries: Option<Vec<RegistryEntry>>,
    task: Option<Task<()>>,
    feedback: Option<(TextKey, bool)>,
}

type RegistryUpdate = (Vec<RegistryEntry>, Result<Option<TextKey>, RegistryError>);

enum Command {
    List,
    Test(RegistryEntry),
    Save(RegistryAddress, RegistryCredentials, Option<RegistryEntry>),
    Remove(RegistryEntry),
}

impl Command {
    async fn run(self, store: &RegistryStore) -> Result<Option<TextKey>, RegistryError> {
        let result = match self {
            Self::List => return Ok(None),
            Self::Test(entry) => {
                store.test(entry).await?;
                gui::GUI_REGISTRIES_TEST_SUCCEEDED
            }
            Self::Save(address, credential, previous) => {
                store.save(address, credential, previous).await?;
                gui::GUI_REGISTRIES_SAVED
            }
            Self::Remove(entry) => {
                store.remove(entry).await?;
                gui::GUI_REGISTRIES_REMOVED
            }
        };
        Ok(Some(result))
    }
}

impl SystemView {
    pub(in crate::screens::system) fn load_registries(&mut self, cx: &mut Context<Self>) {
        self.registry_command(Command::List, cx);
    }

    pub(super) fn registries(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let state = &self.registry_settings;
        let busy = state.task.is_some();
        let locale = screen.locale;
        let palette = screen.palette;
        let available = self.services.is_some() && state.entries.is_some() && !busy;
        let hub = RegistryAddress::docker_hub();
        let entries = state.entries.as_deref().unwrap_or_default();
        settings_card(screen)
            .child(
                row(locale)
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .flex_wrap()
                    .child(
                        settings_heading(
                            screen,
                            gui::GUI_REGISTRIES_TITLE.text(locale),
                            gui::GUI_REGISTRIES_HELP.text(locale),
                        )
                        .flex_1()
                        .min_w(px(225.)),
                    )
                    .child(
                        row(locale)
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("add-registry")
                                    .primary()
                                    .label(gui::GUI_REGISTRIES_ADD.text(locale))
                                    .disabled(!available)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.registry_editor(
                                            None, None, locale, palette, window, cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("refresh-registries")
                                    .outline()
                                    .label(gui::GUI_REGISTRIES_REFRESH.text(locale))
                                    .disabled(busy || self.services.is_none())
                                    .loading(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.load_registries(cx)),
                                    ),
                            ),
                    ),
            )
            .children(state.feedback.as_ref().map(|(feedback, error)| {
                settings_text(
                    screen,
                    Role::Small,
                    feedback.text(locale),
                    if *error {
                        ColorToken::StatusError
                    } else {
                        ColorToken::StatusRunning
                    },
                )
            }))
            .child(registry_row(
                screen,
                hub.clone(),
                entries.iter().find(|entry| entry.address() == &hub),
                available,
                cx,
            ))
            .children(
                entries
                    .iter()
                    .filter(|entry| !entry.address().is_docker_hub())
                    .map(|entry| {
                        registry_row(screen, entry.address().clone(), Some(entry), available, cx)
                    }),
            )
    }

    fn registry_editor(
        &self,
        address: Option<RegistryAddress>,
        current: Option<RegistryEntry>,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.registry_settings.task.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let host = registry_field("", false, window, cx);
        let username = registry_field(
            current.as_ref().map_or("", RegistryEntry::username),
            false,
            window,
            cx,
        );
        let secret = registry_field("", true, window, cx);
        let view = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let valid_address = registry_address(address.as_ref(), &host.read(cx).value()).is_ok();
            let fixed_address = address.clone();
            let host_field = host.clone();
            let valid_credential = RegistryCredentials::new(
                username.read(cx).value().to_string(),
                secret.read(cx).value().to_string(),
            )
            .is_ok();
            let save = view.clone();
            let previous = current.clone();
            let username_field = username.clone();
            let secret_field = secret.clone();
            let title = if address.is_none() {
                gui::GUI_REGISTRIES_ADD
            } else {
                gui::GUI_REGISTRIES_CONFIGURE
            };
            let registry = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(ui_size(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(gui::GUI_REGISTRIES_HOST.text(locale)),
                )
                .child(match address.as_ref() {
                    Some(address) => Role::Mono
                        .latin()
                        .apply(div())
                        .child(technical(address.as_str())),
                    None => div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(Input::new(&host).aria_label(gui::GUI_REGISTRIES_HOST.text(locale)))
                        .child(
                            div()
                                .text_size(ui_size(12.))
                                .text_color(color(palette, ColorToken::TextMuted))
                                .child(gui::GUI_REGISTRIES_HOST_HELP.text(locale)),
                        ),
                });
            dialog_frame(dialog, 600.)
                .overlay_closable(false)
                .title(dialog_heading(locale, palette, title.text(locale)))
                .child(
                    div().flex().flex_col().gap_4().child(registry).child(
                        row(locale)
                            .flex_wrap()
                            .items_start()
                            .gap_3()
                            .child(input_label(
                                &username,
                                gui::GUI_REGISTRIES_USERNAME.text(locale),
                            ))
                            .child(input_label(
                                &secret,
                                gui::GUI_REGISTRIES_SECRET.text(locale),
                            )),
                    ),
                )
                .footer(
                    dialog_actions(locale, palette)
                        .child(
                            Button::new("cancel-registry")
                                .outline()
                                .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("save-registry")
                                .primary()
                                .label(gui::GUI_REGISTRIES_SAVE.text(locale))
                                .disabled(!valid_address || !valid_credential)
                                .on_click(move |_, window, cx| {
                                    let Ok(address) = registry_address(
                                        fixed_address.as_ref(),
                                        &host_field.read(cx).value(),
                                    ) else {
                                        return;
                                    };
                                    let Ok(credential) = RegistryCredentials::new(
                                        username_field.read(cx).value().to_string(),
                                        secret_field.read(cx).value().to_string(),
                                    ) else {
                                        return;
                                    };
                                    let started = save.update(cx, |view, cx| {
                                        view.registry_command(
                                            Command::Save(
                                                address.clone(),
                                                credential,
                                                previous.clone(),
                                            ),
                                            cx,
                                        )
                                    });
                                    if matches!(started, Ok(true)) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    fn confirm_remove_registry(
        &self,
        entry: RegistryEntry,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_REGISTRIES_REMOVE.text(locale),
            Some(gui::GUI_REGISTRIES_REMOVE_HELP.text(locale)),
            &[
                PromptButton::Ok(gui::GUI_REGISTRIES_REMOVE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        cx.spawn(async move |view, cx| {
            if answer.await == Ok(0) {
                let _ = view.update(cx, |view, cx| {
                    view.registry_command(Command::Remove(entry), cx);
                });
            }
        })
        .detach();
    }

    fn registry_command(&mut self, command: Command, cx: &mut Context<Self>) -> bool {
        let Some(services) = &self.services else {
            return false;
        };
        if self.registry_settings.task.is_some() {
            return false;
        }
        self.registry_settings.feedback = None;
        let mut request = services.pending(run(command));
        self.registry_settings.task = Some(cx.spawn(async move |view, cx| {
            let result = request.result().await;
            let _ = view.update(cx, |view, cx| {
                let state = &mut view.registry_settings;
                state.task = None;
                match result {
                    Some(Ok(Ok((entries, status)))) => {
                        state.entries = Some(entries);
                        state.feedback = match status {
                            Ok(key) => key.map(|key| (key, false)),
                            Err(error) => Some((error_text(&error), true)),
                        };
                    }
                    Some(Ok(Err(error))) => {
                        state.entries = None;
                        state.feedback = Some((error_text(&error), true));
                    }
                    _ => state.feedback = Some((gui::GUI_REGISTRIES_FAILED, true)),
                }
                cx.notify();
            });
        }));
        cx.notify();
        true
    }
}

async fn run(
    command: Command,
) -> Result<Result<RegistryUpdate, RegistryError>, crate::services::ServiceError> {
    let result = async {
        let store = RegistryStore::discover().await?;
        let status = command.run(&store).await;
        Ok((store.list().await?, status))
    }
    .await;
    Ok(result)
}

fn registry_address(
    fixed: Option<&RegistryAddress>,
    input: &str,
) -> Result<RegistryAddress, RegistryError> {
    if let Some(address) = fixed {
        return Ok(address.clone());
    }
    let address = RegistryAddress::parse(input.trim())?;
    if address.is_docker_hub() {
        return Err(RegistryError::Address);
    }
    Ok(address)
}

fn registry_field(
    value: &str,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<SystemView>,
) -> Entity<InputState> {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(value.to_owned())
            .masked(masked)
    });
    cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.refresh_windows())
        .detach();
    input
}

fn input_label(input: &Entity<InputState>, label: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(200.))
        .gap_1()
        .child(
            div()
                .text_size(ui_size(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(Input::new(input).aria_label(label))
}

fn registry_row(
    screen: &ScreenContext<'_>,
    address: RegistryAddress,
    entry: Option<&RegistryEntry>,
    available: bool,
    cx: &mut Context<SystemView>,
) -> Div {
    let locale = screen.locale;
    let palette = screen.palette;
    let configure = address.clone();
    let current = entry.cloned();
    row(locale)
        .flex_wrap()
        .justify_between()
        .gap_3()
        .border_t_1()
        .border_color(crate::screens::common::color(
            screen.palette,
            ColorToken::BorderSubtle,
        ))
        .pt_3()
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .children(address.is_docker_hub().then(|| {
                    settings_text(
                        screen,
                        Role::Label,
                        gui::GUI_REGISTRIES_DOCKER_HUB.text(locale),
                        ColorToken::TextPrimary,
                    )
                }))
                .child(settings_text(
                    screen,
                    Role::Mono,
                    technical(address.as_str()),
                    ColorToken::TextPrimary,
                ))
                .child(settings_text(
                    screen,
                    Role::Small,
                    entry.map_or_else(
                        || gui::GUI_REGISTRIES_NO_CREDENTIAL.text(locale).into(),
                        |entry| technical(entry.username()),
                    ),
                    ColorToken::TextMuted,
                )),
        )
        .child(
            row(locale)
                .gap_2()
                .flex_wrap()
                .child(
                    Button::new(SharedString::from(format!(
                        "configure-registry:{}",
                        address.as_str()
                    )))
                    .outline()
                    .label(gui::GUI_REGISTRIES_CONFIGURE.text(locale))
                    .disabled(!available)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.registry_editor(
                            Some(configure.clone()),
                            current.clone(),
                            locale,
                            palette,
                            window,
                            cx,
                        );
                    })),
                )
                .children(entry.cloned().map(|entry| {
                    let testing = entry.clone();
                    row(locale)
                        .gap_2()
                        .child(
                            Button::new(SharedString::from(format!(
                                "test-registry:{}",
                                address.as_str()
                            )))
                            .outline()
                            .label(gui::GUI_REGISTRIES_TEST.text(locale))
                            .disabled(!available)
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    view.registry_command(Command::Test(testing.clone()), cx);
                                },
                            )),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "remove-registry:{}",
                                address.as_str()
                            )))
                            .outline()
                            .label(gui::GUI_REGISTRIES_REMOVE.text(locale))
                            .disabled(!available)
                            .on_click(cx.listener(
                                move |view, _, window, cx| {
                                    view.confirm_remove_registry(entry.clone(), locale, window, cx);
                                },
                            )),
                        )
                })),
        )
}

fn error_text(error: &RegistryError) -> TextKey {
    match error {
        RegistryError::Keychain => gui::GUI_REGISTRIES_KEYCHAIN,
        RegistryError::Changed => gui::GUI_REGISTRIES_CHANGED,
        RegistryError::Cleanup => gui::GUI_REGISTRIES_CLEANUP,
        RegistryError::Challenge => gui::GUI_REGISTRIES_CHALLENGE,
        RegistryError::Connection => gui::GUI_REGISTRIES_TEST_FAILED,
        _ => gui::GUI_REGISTRIES_FAILED,
    }
}
