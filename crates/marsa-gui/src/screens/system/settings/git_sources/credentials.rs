//! Keep the selected Git source visible while editing its Keychain credential.

use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        AppContext as _, Context, Entity, ParentElement, PromptButton, PromptLevel, Styled, Window,
        div,
    },
    gpui_component::{
        Disableable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
    },
};

use crate::screens::common::{
    align, color, dialog_actions, dialog_frame, dialog_heading, resource_input, technical,
};

use super::{GitRepository, GitRepositoryStore, Outcome, SystemView, gui};

impl SystemView {
    pub(super) fn open_git_credentials(
        &mut self,
        repository: GitRepository,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_sources.task.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let username = input(window, cx, false);
        let secret = input(window, cx, true);
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let disabled =
                username.read(cx).value().is_empty() || secret.read(cx).value().is_empty();
            let user = username.clone();
            let password = secret.clone();
            let owner = owner.clone();
            let source = repository.clone();
            dialog_frame(dialog, 540.)
                .overlay_closable(false)
                .title(dialog_heading(
                    locale,
                    palette,
                    gui::GUI_GIT_CREDENTIALS.text(locale),
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(align(div(), locale).child(repository.config.name.clone()))
                        .child(
                            align(div(), locale)
                                .text_color(color(palette, ColorToken::TextMuted))
                                .child(technical(&repository.config.url)),
                        )
                        .child(
                            align(div(), locale).child(gui::GUI_GIT_CREDENTIAL_HELP.text(locale)),
                        )
                        .child(labeled(
                            &username,
                            gui::GUI_GIT_CREDENTIAL_USER.text(locale),
                            locale,
                        ))
                        .child(labeled(
                            &secret,
                            gui::GUI_GIT_CREDENTIAL_SECRET.text(locale),
                            locale,
                        )),
                )
                .footer(
                    dialog_actions(locale, palette)
                        .child(
                            Button::new("cancel-git-credential")
                                .outline()
                                .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("save-git-credential")
                                .primary()
                                .label(gui::GUI_GIT_CREDENTIAL_SAVE.text(locale))
                                .disabled(disabled)
                                .on_click(move |_, window, cx| {
                                    let username = user.read(cx).value().to_string();
                                    let secret = password.read(cx).value().to_string();
                                    password.update(cx, |password, cx| {
                                        password.set_value(String::new(), window, cx);
                                    });
                                    window.close_dialog(cx);
                                    let source = source.clone();
                                    let _ = owner.update(cx, |this, cx| {
                                        this.git_source_task(
                                            async move {
                                                let store = GitRepositoryStore::discover()?;
                                                let result = store
                                                    .save_https_credential(
                                                        &source.id,
                                                        &source.config.url,
                                                        username,
                                                        secret,
                                                    )
                                                    .await;
                                                Outcome::change(
                                                    &store,
                                                    result,
                                                    gui::GUI_GIT_CREDENTIAL_SAVED,
                                                )
                                                .await
                                            },
                                            cx,
                                        );
                                    });
                                }),
                        ),
                )
        });
    }

    pub(super) fn confirm_remove_git_credential(
        &mut self,
        repository: GitRepository,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_GIT_CREDENTIAL_REMOVE.text(locale),
            Some(gui::GUI_GIT_CREDENTIAL_REMOVE_HELP.text(locale)),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_REMOVE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        cx.spawn(async move |owner, cx| {
            if answer.await == Ok(0) {
                let _ = owner.update(cx, |this, cx| {
                    this.git_source_task(
                        async move {
                            let store = GitRepositoryStore::discover()?;
                            let result = store
                                .remove_https_credential(&repository.id, &repository.config.url)
                                .await;
                            Outcome::change(&store, result, gui::GUI_GIT_CREDENTIAL_REMOVED).await
                        },
                        cx,
                    );
                });
            }
        })
        .detach();
    }
}

fn input(window: &mut Window, cx: &mut Context<SystemView>, masked: bool) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).masked(masked));
    cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.refresh_windows())
        .detach();
    input
}

fn labeled(field: &Entity<InputState>, label: &'static str, locale: Locale) -> marsa_ui::gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(align(div(), locale).child(label))
        .child(resource_input(field).aria_label(label))
}
