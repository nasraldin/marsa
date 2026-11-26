//! Review independent SSH host trust and an explicitly chosen owner key.

use std::path::PathBuf;

use marsa_client::git_sources::{git_ssh_host_key_fingerprint, review_git_ssh_identity};
use marsa_ui::{
    ColorToken, Locale,
    gpui::{Context, Div, EntityId, ParentElement, PathPromptOptions, Styled, div},
    gpui_component::{Disableable as _, button::Button},
    tokens::typography::Role,
};

use crate::screens::common::{ScreenContext, resource_input, row, technical};

use super::{Editor, Outcome, SystemView, gui, settings_text};

#[derive(Clone)]
pub(super) struct IdentityReview {
    pub(super) file: PathBuf,
    pub(super) fingerprint: String,
}

impl SystemView {
    pub(super) fn ssh_git_editor(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        busy: bool,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let host_key = &editor.fields[3];
        let owner = editor.fields[0].entity_id();
        let fingerprint = git_ssh_host_key_fingerprint(host_key.read(cx).value().trim());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(settings_text(
                screen,
                Role::Label,
                gui::GUI_GIT_SSH_HOST_KEY.text(locale),
                ColorToken::TextPrimary,
            ))
            .child(settings_text(
                screen,
                Role::Small,
                gui::GUI_GIT_SSH_HOST_KEY_HELP.text(locale),
                ColorToken::TextMuted,
            ))
            .child(
                resource_input(host_key)
                    .disabled(busy)
                    .aria_label(gui::GUI_GIT_SSH_HOST_KEY.text(locale)),
            )
            .children(fingerprint.as_ref().map(|fingerprint| {
                settings_text(
                    screen,
                    Role::Mono,
                    technical(fingerprint),
                    ColorToken::TextPrimary,
                )
            }))
            .child(
                row(locale)
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("git-choose-ssh-identity")
                            .outline()
                            .label(gui::GUI_GIT_SSH_CHOOSE_KEY.text(locale))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.choose_git_ssh_identity(owner, locale, cx);
                            })),
                    )
                    .children(editor.ssh_identity.as_ref().map(|identity| {
                        settings_text(
                            screen,
                            Role::Small,
                            technical(&identity.file.to_string_lossy()),
                            ColorToken::TextPrimary,
                        )
                        .min_w_0()
                        .truncate()
                    })),
            )
            .children(editor.ssh_identity.as_ref().map(|identity| {
                settings_text(
                    screen,
                    Role::Mono,
                    technical(&identity.fingerprint),
                    ColorToken::TextPrimary,
                )
            }))
            .child(settings_text(
                screen,
                Role::Small,
                gui::GUI_GIT_SSH_KEY_HELP.text(locale),
                ColorToken::TextMuted,
            ))
    }

    fn choose_git_ssh_identity(
        &mut self,
        editor: EntityId,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        if self.git_sources.task.is_some() {
            return;
        }
        let chooser = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(gui::GUI_GIT_SSH_CHOOSE_KEY.text(locale).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chooser.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if !this.git_sources.editor.as_ref().is_some_and(|current| {
                    current.fields[0].entity_id() == editor && current.is_ssh(cx)
                }) {
                    return;
                }
                this.git_source_task(
                    async move {
                        let fingerprint = review_git_ssh_identity(&file).await?;
                        Ok(Outcome::ReviewedIdentity(
                            editor,
                            IdentityReview { file, fingerprint },
                        ))
                    },
                    cx,
                );
            });
        })
        .detach();
    }
}
