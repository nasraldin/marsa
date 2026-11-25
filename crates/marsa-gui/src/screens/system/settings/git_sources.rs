//! Owner-managed Git sources and a deliberate checkout action.

mod credentials;

use std::{future::Future, path::PathBuf};

use marsa_client::git_sources::{
    GitRepository, GitRepositoryConfig, GitRepositoryStore, GitSourceError,
};
use marsa_ui::{
    ColorToken, TextKey,
    gpui::{
        AppContext as _, Context, Div, ElementId, Entity, ParentElement, PathPromptOptions,
        PromptButton, PromptLevel, Styled, Subscription, Task, Window, div, px,
    },
    gpui_component::{
        Disableable as _,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, settings_card, settings_heading, settings_text};
use crate::screens::common::{ScreenContext, color, resource_input, row, technical};

#[derive(Default)]
pub(in crate::screens::system) struct GitSources {
    entries: Option<Vec<GitRepository>>,
    task: Option<Task<()>>,
    editor: Option<Editor>,
    feedback: Option<(TextKey, ColorToken)>,
}

struct Editor {
    id: Option<String>,
    fields: [Entity<InputState>; 3],
    _observers: [Subscription; 3],
}

enum Outcome {
    Listed(Vec<GitRepository>),
    Changed(Vec<GitRepository>, TextKey, ColorToken),
    Tested,
    CheckedOut(PathBuf),
}

impl Outcome {
    async fn change<T>(
        store: &GitRepositoryStore,
        result: Result<T, GitSourceError>,
        success: TextKey,
    ) -> Result<Self, GitSourceError> {
        let (key, color) = match result {
            Ok(_) => (success, ColorToken::TextPrimary),
            Err(error @ GitSourceError::CredentialCleanup) => {
                (error_key(&error), ColorToken::StatusWarning)
            }
            Err(error) => return Err(error),
        };
        Ok(Self::Changed(store.list().await?, key, color))
    }
}

impl Editor {
    fn new(
        repo: Option<&GitRepository>,
        window: &mut Window,
        cx: &mut Context<SystemView>,
    ) -> Self {
        let values = repo.map_or(["", "", "main"], |repo| {
            [&repo.config.name, &repo.config.url, &repo.config.branch]
        });
        let fields = values.map(|value| {
            let field = cx.new(|cx| InputState::new(window, cx));
            field.update(cx, |field, cx| {
                field.set_value(value.to_owned(), window, cx)
            });
            field
        });
        let _observers = fields.each_ref().map(|field| {
            cx.subscribe(field, |view, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) && view.git_sources.feedback.take().is_some()
                {
                    cx.notify();
                }
            })
        });
        Self {
            id: repo.map(|repo| repo.id.clone()),
            fields,
            _observers,
        }
    }

    fn config(&self, cx: &Context<SystemView>) -> GitRepositoryConfig {
        let [name, url, branch] = self
            .fields
            .each_ref()
            .map(|field| field.read(cx).value().trim().to_owned());
        GitRepositoryConfig { name, url, branch }
    }
}

impl SystemView {
    pub(in crate::screens::system) fn load_git_sources(&mut self, cx: &mut Context<Self>) {
        self.git_source_task(
            async {
                Ok(Outcome::Listed(
                    GitRepositoryStore::discover()?.list().await?,
                ))
            },
            cx,
        );
    }

    fn git_source_task(
        &mut self,
        work: impl Future<Output = Result<Outcome, GitSourceError>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        if self.git_sources.task.is_some() {
            return;
        }
        self.git_sources.feedback = None;
        let mut pending = services.pending(async { Ok(work.await) });
        self.git_sources.task = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                let panel = &mut this.git_sources;
                panel.task = None;
                panel.feedback = match result {
                    Some(Ok(Ok(Outcome::Listed(entries)))) => {
                        panel.entries = Some(entries);
                        None
                    }
                    Some(Ok(Ok(Outcome::Changed(entries, key, color)))) => {
                        panel.entries = Some(entries);
                        panel.editor = None;
                        Some((key, color))
                    }
                    Some(Ok(Ok(Outcome::Tested))) => {
                        Some((gui::GUI_GIT_BRANCH_AVAILABLE, ColorToken::TextPrimary))
                    }
                    Some(Ok(Ok(Outcome::CheckedOut(path)))) => {
                        cx.reveal_path(&path);
                        Some((gui::GUI_GIT_CHECKOUT_COMPLETE, ColorToken::TextPrimary))
                    }
                    Some(Ok(Err(error))) => Some((error_key(&error), ColorToken::StatusWarning)),
                    _ => Some((gui::GUI_GIT_STORE_FAILED, ColorToken::StatusWarning)),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn git_submit(&mut self, test: bool, cx: &mut Context<Self>) {
        let Some(editor) = &self.git_sources.editor else {
            return;
        };
        let config = editor.config(cx);
        let id = editor.id.clone();
        if test {
            let saved = self
                .git_sources
                .entries
                .as_deref()
                .and_then(|entries| {
                    entries.iter().find(|entry| {
                        id.as_ref() == Some(&entry.id)
                            && config.url == entry.config.url
                            && config.branch == entry.config.branch
                    })
                })
                .cloned();
            if let Some(saved) = saved {
                self.test_saved_git_source(saved, cx);
            } else {
                self.test_git_source(config, cx);
            }
            return;
        }
        self.git_source_task(
            async move {
                let store = GitRepositoryStore::discover()?;
                let result = store.save(id.as_deref(), config).await;
                Outcome::change(&store, result, gui::GUI_GIT_SAVED).await
            },
            cx,
        );
    }

    fn test_git_source(&mut self, config: GitRepositoryConfig, cx: &mut Context<Self>) {
        self.git_source_task(
            async move {
                config.test().await?;
                Ok(Outcome::Tested)
            },
            cx,
        );
    }

    fn test_saved_git_source(&mut self, source: GitRepository, cx: &mut Context<Self>) {
        self.git_source_task(
            async move {
                source.test().await?;
                Ok(Outcome::Tested)
            },
            cx,
        );
    }

    fn checkout_git_source(
        &mut self,
        source: GitRepository,
        locale: marsa_ui::Locale,
        cx: &mut Context<Self>,
    ) {
        let chooser = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(gui::GUI_GIT_CHOOSE_FOLDER.text(locale).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chooser.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.git_source_task(
                    async move { Ok(Outcome::CheckedOut(source.checkout(&path).await?)) },
                    cx,
                )
            });
        })
        .detach();
    }

    pub(super) fn git_sources(&self, screen: &ScreenContext<'_>, cx: &mut Context<Self>) -> Div {
        let locale = screen.locale;
        let panel = &self.git_sources;
        let busy = panel.task.is_some() || self.services.is_none();
        let header = row(locale)
            .flex_wrap()
            .justify_between()
            .gap_3()
            .child(settings_heading(
                screen,
                gui::GUI_GIT_SAVED_SOURCES.text(locale),
                gui::GUI_GIT_SOURCE_HELP.text(locale),
            ))
            .child(
                Button::new("add-git-source")
                    .primary()
                    .label(gui::GUI_GIT_ADD.text(locale))
                    .disabled(busy || panel.editor.is_some())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.git_sources.editor = Some(Editor::new(None, window, cx));
                        this.git_sources.feedback = None;
                        cx.notify();
                    })),
            );
        let mut card = settings_card(screen).child(header);
        if panel.task.is_some() {
            card = card.child(settings_text(
                screen,
                Role::Small,
                gui::GUI_DESKTOP_WORKING.text(locale),
                ColorToken::TextMuted,
            ));
        }
        if let Some((key, color)) = panel.feedback {
            card = card.child(settings_text(screen, Role::Small, key.text(locale), color));
        }
        if let Some(editor) = &panel.editor {
            card = card.child(self.git_source_editor(editor, screen, busy, cx));
        }
        if let Some(entries) = &panel.entries {
            if entries.is_empty() && panel.editor.is_none() {
                card = card.child(settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_GIT_EMPTY.text(locale),
                    ColorToken::TextMuted,
                ));
            }
            card =
                card.children(entries.iter().map(|repo| {
                    self.git_source_row(repo, screen, busy || panel.editor.is_some(), cx)
                }));
        }
        card
    }

    fn git_source_editor(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        busy: bool,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let fields = row(locale).flex_wrap().gap_3().children(
            [
                (gui::GUI_GIT_NAME, 150.),
                (gui::GUI_GIT_URL, 220.),
                (gui::GUI_GIT_BRANCH, 110.),
            ]
            .into_iter()
            .zip(editor.fields.iter())
            .map(|((key, width), field)| {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(width))
                    .gap_1()
                    .child(settings_text(
                        screen,
                        Role::Label,
                        key.text(locale),
                        ColorToken::TextPrimary,
                    ))
                    .child(
                        resource_input(field)
                            .disabled(busy)
                            .aria_label(key.text(locale)),
                    )
            }),
        );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .border_t_1()
            .border_color(color(screen.palette, ColorToken::BorderSubtle))
            .pt_3()
            .child(fields)
            .child(
                row(locale)
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("git-editor-test")
                            .outline()
                            .label(gui::GUI_GIT_TEST.text(locale))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.git_submit(true, cx))),
                    )
                    .child(
                        row(locale)
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("git-editor-cancel")
                                    .ghost()
                                    .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.git_sources.editor = None;
                                        this.git_sources.feedback = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("git-editor-save")
                                    .primary()
                                    .label(gui::GUI_ENVIRONMENT_SAVE.text(locale))
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.git_submit(false, cx)),
                                    ),
                            ),
                    ),
            )
    }

    fn git_source_row(
        &self,
        repo: &GitRepository,
        screen: &ScreenContext<'_>,
        busy: bool,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let edit = repo.clone();
        let selected = repo.clone();
        let test = repo.clone();
        let credentials = repo.clone();
        let remove_credentials = repo.clone();
        let palette = screen.palette;
        let id = repo.id.clone();
        let button = |action: &'static str, title: TextKey| {
            Button::new((
                ElementId::from(action),
                marsa_ui::gpui::SharedString::from(repo.id.clone()),
            ))
            .outline()
            .label(title.text(locale))
            .disabled(busy)
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_1()
            .border_color(color(screen.palette, ColorToken::BorderSubtle))
            .pt_3()
            .child(settings_text(
                screen,
                Role::Label,
                repo.config.name.clone(),
                ColorToken::TextPrimary,
            ))
            .child(settings_text(
                screen,
                Role::Mono,
                technical(&repo.config.url),
                ColorToken::TextMuted,
            ))
            .child(settings_text(
                screen,
                Role::Mono,
                technical(&repo.config.branch),
                ColorToken::TextMuted,
            ))
            .child(
                row(locale)
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button("git-checkout", gui::GUI_GIT_CHECKOUT).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.checkout_git_source(selected.clone(), locale, cx)
                            },
                        )),
                    )
                    .child(button("git-test", gui::GUI_GIT_TEST).on_click(cx.listener(
                        move |this, _, _, cx| this.test_saved_git_source(test.clone(), cx),
                    )))
                    .child(
                        button("git-edit", gui::GUI_ENVIRONMENT_EDIT).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.git_sources.editor =
                                    Some(Editor::new(Some(&edit), window, cx));
                                this.git_sources.feedback = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("git-credentials", gui::GUI_GIT_CREDENTIALS).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_git_credentials(
                                    credentials.clone(),
                                    locale,
                                    palette,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .children(repo.has_https_credential().then(|| {
                        button("git-remove-credential", gui::GUI_GIT_CREDENTIAL_REMOVE).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.confirm_remove_git_credential(
                                    remove_credentials.clone(),
                                    locale,
                                    window,
                                    cx,
                                );
                            }),
                        )
                    }))
                    .child(
                        button("git-remove", gui::GUI_DESKTOP_REMOVE).on_click(cx.listener(
                            move |_, _, window, cx| {
                                let id = id.clone();
                                let answer = window.prompt(
                                    PromptLevel::Warning,
                                    gui::GUI_GIT_REMOVE_TITLE.text(locale),
                                    Some(gui::GUI_GIT_REMOVE_HELP.text(locale)),
                                    &[
                                        PromptButton::Ok(
                                            gui::GUI_DESKTOP_REMOVE.text(locale).into(),
                                        ),
                                        PromptButton::Cancel(
                                            gui::GUI_DESKTOP_CANCEL.text(locale).into(),
                                        ),
                                    ],
                                    cx,
                                );
                                cx.spawn(async move |this, cx| {
                                    if answer.await == Ok(0) {
                                        let _ = this.update(cx, |this, cx| {
                                            this.git_source_task(
                                                async move {
                                                    let store = GitRepositoryStore::discover()?;
                                                    let result = store.remove(&id).await;
                                                    Outcome::change(
                                                        &store,
                                                        result,
                                                        gui::GUI_GIT_REMOVED,
                                                    )
                                                    .await
                                                },
                                                cx,
                                            )
                                        });
                                    }
                                })
                                .detach();
                            },
                        )),
                    ),
            )
    }
}

fn error_key(error: &GitSourceError) -> TextKey {
    match error {
        GitSourceError::Invalid => gui::GUI_GIT_INVALID,
        GitSourceError::Destination => gui::GUI_GIT_BAD_FOLDER,
        GitSourceError::Timeout => gui::GUI_GIT_TIMEOUT,
        GitSourceError::Git => gui::GUI_GIT_FAILED,
        GitSourceError::BranchMissing => gui::GUI_GIT_BRANCH_MISSING,
        GitSourceError::CredentialInvalid => gui::GUI_GIT_CREDENTIAL_INVALID,
        GitSourceError::CredentialUnavailable => gui::GUI_GIT_CREDENTIAL_UNAVAILABLE,
        GitSourceError::CredentialCleanup => gui::GUI_GIT_CREDENTIAL_CLEANUP,
        GitSourceError::NotFound | GitSourceError::Store(_) => gui::GUI_GIT_STORE_FAILED,
    }
}
