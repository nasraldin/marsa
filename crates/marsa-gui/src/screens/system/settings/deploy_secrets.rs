//! Collect the exact Compose scope and keep secret values out of Settings state.

use marsa_client::{compose::valid_project_name, docker::DeploySecretStore};
use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        AppContext as _, Context, Div, Entity, ParentElement as _, PathPromptOptions, Styled as _,
        Task, Window, div,
    },
    gpui_component::{
        Disableable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{Input, InputEvent, InputState},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use crate::screens::common::{ScreenContext, dialog_actions, dialog_frame, dialog_heading, row};

use super::{SystemView, settings_card, settings_heading, settings_text};

#[derive(Default)]
pub(in crate::screens::system) struct DeploySecrets {
    task: Option<Task<()>>,
    result: Option<(TextKey, bool)>,
}

impl SystemView {
    pub(super) fn deploy_secret_settings(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let palette = screen.palette;
        let busy = self.deploy_secrets.task.is_some() || self.services.is_none();
        settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_DEPLOY_SECRETS_TITLE.text(locale),
                gui::GUI_DEPLOY_SECRETS_HELP.text(locale),
            ))
            .child(
                row(locale)
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("save-compose-keychain")
                            .outline()
                            .disabled(busy)
                            .label(gui::GUI_DEPLOY_SECRETS_SAVE.text(locale))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.deploy_secret_editor(false, locale, palette, window, cx);
                            })),
                    )
                    .child(
                        Button::new("remove-compose-keychain")
                            .outline()
                            .disabled(busy)
                            .label(gui::GUI_DEPLOY_SECRETS_REMOVE.text(locale))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.deploy_secret_editor(true, locale, palette, window, cx);
                            })),
                    ),
            )
            .children(self.deploy_secrets.result.map(|(key, failed)| {
                settings_text(
                    screen,
                    Role::Small,
                    key.text(locale),
                    if failed {
                        ColorToken::StatusError
                    } else {
                        ColorToken::StatusRunning
                    },
                )
            }))
    }

    fn deploy_secret_editor(
        &mut self,
        remove: bool,
        locale: Locale,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.deploy_secrets.task.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let project = field(window, cx, false);
        let name = field(window, cx, false);
        let value = (!remove).then(|| field(window, cx, true));
        let title = if remove {
            gui::GUI_DEPLOY_SECRETS_REMOVE
        } else {
            gui::GUI_DEPLOY_SECRETS_SAVE
        };
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let valid = valid_project_name(project.read(cx).value().trim())
                && !name.read(cx).value().trim().is_empty()
                && value
                    .as_ref()
                    .is_none_or(|state| !state.read(cx).value().is_empty());
            let submit = Button::new("submit-deploy-secret")
                .label(title.text(locale))
                .disabled(!valid);
            let submit = if remove {
                submit.danger()
            } else {
                submit.primary()
            };
            let (edit_project, edit_name, edit_value) =
                (project.clone(), name.clone(), value.clone());
            let owner = owner.clone();
            dialog_frame(dialog, 520.)
                .overlay_closable(false)
                .title(dialog_heading(locale, palette, title.text(locale)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(labeled_field(
                            &project,
                            gui::GUI_DESKTOP_STACK_NAME.text(locale),
                        ))
                        .child(labeled_field(
                            &name,
                            gui::GUI_DEPLOY_SECRETS_NAME.text(locale),
                        ))
                        .children(value.as_ref().map(|value| {
                            labeled_field(value, gui::GUI_DEPLOY_SECRETS_VALUE.text(locale))
                        }))
                        .children(remove.then(|| gui::GUI_DEPLOY_SECRETS_REMOVE_HELP.text(locale))),
                )
                .footer(
                    dialog_actions(locale, palette)
                        .child(
                            Button::new("cancel-deploy-secret")
                                .outline()
                                .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(submit.on_click(move |_, window, cx| {
                            let project = edit_project.read(cx).value().trim().to_owned();
                            let name = edit_name.read(cx).value().trim().to_owned();
                            let value = edit_value
                                .as_ref()
                                .map(|state| state.read(cx).value().to_string());
                            window.close_dialog(cx);
                            let _ = owner.update(cx, |this, cx| {
                                this.apply_deploy_secret(project, name, value, locale, cx);
                            });
                        })),
                )
        });
    }

    fn apply_deploy_secret(
        &mut self,
        project: String,
        name: String,
        value: Option<String>,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.services.clone() else {
            return;
        };
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(gui::GUI_DEPLOY_SECRETS_CHOOSE_FILE.text(locale).into()),
        });
        self.deploy_secrets.result = None;
        self.deploy_secrets.task = Some(cx.spawn(async move |owner, cx| {
            let outcome = if let Ok(Ok(Some(paths))) = picker.await
                && let Some(file) = paths.into_iter().next()
            {
                let mut result = service.pending(async move {
                    Ok(async move {
                        let store = DeploySecretStore::for_project(&file, &project).await?;
                        match value {
                            Some(value) => {
                                store.save(&name, value).await?;
                                Ok::<_, marsa_client::docker::DeploySecretError>(
                                    gui::GUI_DEPLOY_SECRETS_SAVED,
                                )
                            }
                            None => {
                                store.remove(&name).await?;
                                Ok(gui::GUI_DEPLOY_SECRETS_REMOVED)
                            }
                        }
                    }
                    .await)
                });
                Some(match result.result().await {
                    Some(Ok(Ok(key))) => (key, false),
                    _ => (gui::GUI_DEPLOY_SECRETS_FAILED, true),
                })
            } else {
                None
            };
            let _ = owner.update(cx, |this, cx| {
                this.deploy_secrets.task = None;
                this.deploy_secrets.result = outcome;
                cx.refresh_windows();
            });
        }));
        cx.refresh_windows();
    }
}

fn field(window: &mut Window, cx: &mut Context<SystemView>, masked: bool) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).masked(masked));
    cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.refresh_windows())
        .detach();
    input
}

fn labeled_field(input: &Entity<InputState>, label: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(label)
        .child(Input::new(input).aria_label(label))
}
