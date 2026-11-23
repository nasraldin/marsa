//! Connection settings keep saved endpoints separate from Docker CLI discovery.

mod editor;
mod observations;
mod vm;

use std::{future::Future, sync::Arc};

use editor::{Appearance, Editor, EditorTarget};

use marsa_client::docker::{
    ManagedEndpoint, ManagedEndpointError, ManagedEndpointStore, ManagedEndpointTransport,
    VerifiedEngine,
};
use marsa_ui::{
    ColorToken, Locale, TextKey,
    gpui::{
        Context, Div, ParentElement as _, PromptButton, PromptLevel, SharedString, Styled as _,
        Task, Window, div, px,
    },
    gpui_component::{
        Disableable as _, IconName, WindowExt as _,
        button::{Button, ButtonVariants as _},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, settings_card, settings_heading, settings_text};
use crate::screens::{
    common::{ScreenContext, color, row, technical},
    external_inventory::{Environment, ExternalWorkspace},
    system::SelectContext,
};

#[derive(Default)]
pub(in crate::screens::system) struct EnvironmentSettings {
    saved: Option<Vec<ManagedEndpoint>>,
    editor: Option<Editor>,
    task: Option<Task<()>>,
    feedback: Option<Feedback>,
}

enum Feedback {
    Verified(VerifiedEngine),
    Connected,
    Saved,
    Removed,
    Failed(TextKey),
}

enum Update {
    Listed(Vec<ManagedEndpoint>),
    Tested(VerifiedEngine),
    MarsaTest(bool),
    Changed(Vec<ManagedEndpoint>, Feedback),
    Closed(Vec<ManagedEndpoint>),
}

impl SystemView {
    pub(crate) fn refresh_external_inventory(&mut self, cx: &mut Context<Self>) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        if self.external_inventory_task.is_some() {
            return;
        }
        let mut pending = services.external_inventory();
        self.external_inventory_task = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Ok(inventories)) = result {
                    this.external_inventory = inventories
                        .into_iter()
                        .map(|inventory| Arc::new(inventory.into()))
                        .collect();
                }
                this.external_inventory_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(in crate::screens::system) fn refresh_environments(&mut self, cx: &mut Context<Self>) {
        self.refresh_external_inventory(cx);
        self.update_environments(
            async {
                Ok(Update::Listed(
                    ManagedEndpointStore::discover()?.list().await?,
                ))
            },
            None,
            cx,
        );
    }

    fn update_environments(
        &mut self,
        work: impl Future<Output = Result<Update, ManagedEndpointError>> + Send + 'static,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(services) = self.services.as_ref() else {
            return;
        };
        if self.environment_settings.task.is_some() {
            return;
        }
        self.environment_settings.feedback = None;
        let mut pending = services.pending(async { Ok(work.await) });
        let window = window.map(|window| window.window_handle());
        self.environment_settings.task = Some(cx.spawn(async move |this, cx| {
            let result = pending.result().await;
            let close = matches!(&result, Some(Ok(Ok(Update::Closed(_)))));
            let _ = this.update(cx, |this, cx| {
                let panel = &mut this.environment_settings;
                panel.task = None;
                match result {
                    Some(Ok(Ok(Update::Listed(saved)))) => panel.saved = Some(saved),
                    Some(Ok(Ok(Update::Tested(engine)))) => {
                        panel.feedback = Some(Feedback::Verified(engine))
                    }
                    Some(Ok(Ok(Update::MarsaTest(connected)))) => {
                        panel.feedback = Some(if connected {
                            Feedback::Connected
                        } else {
                            Feedback::Failed(gui::GUI_ENVIRONMENT_TEST_FAILED)
                        });
                    }
                    Some(Ok(Ok(Update::Changed(saved, feedback)))) => {
                        panel.saved = Some(saved);
                        panel.feedback = Some(feedback);
                        this.external_inventory_task = None;
                        this.refresh_external_inventory(cx);
                    }
                    Some(Ok(Ok(Update::Closed(saved)))) => {
                        panel.editor = None;
                        panel.saved = Some(saved);
                        panel.feedback = Some(Feedback::Saved);
                        this.external_inventory_task = None;
                        this.refresh_external_inventory(cx);
                    }
                    Some(Ok(Err(error))) => {
                        panel.feedback = Some(Feedback::Failed(error_text(&error)))
                    }
                    _ => panel.feedback = Some(Feedback::Failed(gui::GUI_ENVIRONMENT_STORE_ERROR)),
                }
                cx.notify();
            });
            if close && let Some(window) = window {
                let _ = window.update(cx, |_, window, cx| window.close_dialog(cx));
            }
        }));
        cx.notify();
    }

    fn environment_submit(&mut self, test: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.environment_settings.editor else {
            return;
        };
        if matches!(editor.target, EditorTarget::Builtin) {
            if let Some(services) = &self.services {
                let docker = services.resource_docker(None);
                self.update_environments(
                    async move { Ok(Update::MarsaTest(docker.containers().await.is_ok())) },
                    Some(window),
                    cx,
                );
            }
            return;
        }
        if editor.target.is_discovered()
            && let Some(workspace) = editor.target.workspace()
        {
            let endpoint = workspace.endpoint.clone();
            self.update_environments(
                async move { Ok(Update::Tested(endpoint.test_connection().await?)) },
                Some(window),
                cx,
            );
            return;
        }
        let id = editor.id.clone();
        let config = match editor.config(cx) {
            Ok(config) => config,
            Err(key) => {
                self.environment_settings.feedback = Some(Feedback::Failed(key));
                cx.notify();
                return;
            }
        };
        self.update_environments(
            async move {
                if test {
                    return Ok(Update::Tested(config.test().await?));
                }
                let store = ManagedEndpointStore::discover()?;
                if let Some(id) = id {
                    store.edit(&id, config).await?;
                } else {
                    store.add(config).await?;
                }
                Ok(Update::Closed(store.list().await?))
            },
            Some(window),
            cx,
        );
    }

    fn confirm_environment_removal(
        &mut self,
        id: String,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            gui::GUI_ENVIRONMENT_REMOVE_TITLE.text(locale),
            Some(gui::GUI_ENVIRONMENT_REMOVE_HELP.text(locale)),
            &[
                PromptButton::Ok(gui::GUI_DESKTOP_REMOVE.text(locale).into()),
                PromptButton::Cancel(gui::GUI_DESKTOP_CANCEL.text(locale).into()),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    this.update_environments(
                        async move {
                            let store = ManagedEndpointStore::discover()?;
                            store.remove(&id).await?;
                            Ok(Update::Changed(store.list().await?, Feedback::Removed))
                        },
                        None,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    pub(super) fn external_docker(
        &self,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let panel = &self.environment_settings;
        let busy = panel.task.is_some();
        let add_appearance = Appearance::from(screen);
        let builtin_appearance = add_appearance.clone();
        let header = row(locale)
            .items_center()
            .justify_between()
            .gap_3()
            .flex_wrap()
            .child(settings_heading(
                screen,
                gui::GUI_ENVIRONMENT_SAVED_CONNECTIONS.text(locale),
                gui::GUI_ENVIRONMENT_SAVED_HELP.text(locale),
            ))
            .child(
                row(locale)
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("refresh-environments")
                            .outline()
                            .label(gui::GUI_DESKTOP_REFRESH.text(locale))
                            .disabled(busy || self.services.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_environments(cx))),
                    )
                    .child(
                        Button::new("add-environment")
                            .primary()
                            .icon(IconName::Plus)
                            .label(gui::GUI_ENVIRONMENT_ADD.text(locale))
                            .disabled(busy || self.services.is_none() || panel.editor.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_environment_editor(
                                    EditorTarget::Add,
                                    add_appearance.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
        let mut saved = settings_card(screen).child(header);
        if busy {
            saved = saved.child(settings_text(
                screen,
                Role::Small,
                gui::GUI_DESKTOP_WORKING.text(locale),
                ColorToken::TextMuted,
            ));
        }
        if let Some(feedback) = &panel.feedback {
            saved = saved.child(feedback_text(screen, feedback));
        }
        if let Some(entries) = &panel.saved {
            if entries.is_empty() && panel.editor.is_none() {
                saved = saved.child(settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_ENVIRONMENT_NO_SAVED.text(locale),
                    ColorToken::TextMuted,
                ));
            }
            for entry in entries {
                saved = saved.child(self.saved_environment(entry, screen, cx));
            }
        }
        let builtin = settings_card(screen)
            .child(settings_heading(
                screen,
                gui::GUI_ENVIRONMENT_BUILTIN.text(locale),
                gui::GUI_ENVIRONMENT_BUILTIN_HELP.text(locale),
            ))
            .child(
                connection_row(screen)
                    .child(connection_label(
                        screen,
                        "Marsa",
                        gui::GUI_ENVIRONMENT_BUILTIN_ENGINE.text(locale),
                        None,
                    ))
                    .child(
                        Button::new("edit-marsa-environment")
                            .outline()
                            .label(gui::GUI_ENVIRONMENT_EDIT.text(locale))
                            .disabled(busy || panel.editor.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_environment_editor(
                                    EditorTarget::Builtin,
                                    builtin_appearance.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(crate::desktop::status::compact_badge(
                        locale,
                        screen.palette,
                        screen.state.engine().map(|engine| engine.state()),
                    ))
                    .child(
                        Button::new("use-marsa-environment")
                            .outline()
                            .label(gui::GUI_ENVIRONMENT_USE.text(locale))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(SelectContext(Environment::Marsa))
                            })),
                    ),
            );
        let mut discovered = settings_card(screen).child(settings_heading(
            screen,
            gui::GUI_ENVIRONMENT_DISCOVERED_CONNECTIONS.text(locale),
            gui::GUI_ENVIRONMENT_DISCOVERED_HELP.text(locale),
        ));
        let mut sources = self
            .external_inventory
            .iter()
            .filter(|entry| entry.endpoint.managed_id().is_none())
            .peekable();
        if sources.peek().is_none() {
            let state = if self.external_inventory_task.is_some() {
                gui::GUI_DESKTOP_SCANNING_EXTERNAL_DOCKER
            } else {
                gui::GUI_DESKTOP_NO_EXTERNAL_DOCKER
            };
            discovered = discovered.child(settings_text(
                screen,
                Role::Small,
                state.text(locale),
                ColorToken::TextMuted,
            ));
        }
        for (i, engine) in sources.enumerate() {
            let edit = Arc::clone(engine);
            let appearance = Appearance::from(screen);
            discovered = discovered.child(
                connection_row(screen)
                    .child(connection_label(
                        screen,
                        &engine.source,
                        &engine.endpoint.address(),
                        None,
                    ))
                    .child(connection_status(screen, Some(engine)))
                    .child(
                        Button::new(("edit-discovered-environment", i))
                            .outline()
                            .label(gui::GUI_ENVIRONMENT_EDIT.text(locale))
                            .disabled(busy || panel.editor.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_environment_editor(
                                    EditorTarget::Discovered(edit.clone()),
                                    appearance.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(select_button(
                        screen,
                        ("discovered-environment", i),
                        engine,
                        cx,
                    )),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(builtin)
            .child(saved)
            .child(discovered)
    }

    fn saved_environment(
        &self,
        entry: &ManagedEndpoint,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let workspace = self
            .external_inventory
            .iter()
            .find(|source| source.endpoint.managed_id() == Some(&entry.id));
        let disabled =
            self.environment_settings.task.is_some() || self.environment_settings.editor.is_some();
        let address = match &entry.config.transport {
            ManagedEndpointTransport::Unix { socket } => socket.to_string_lossy(),
            ManagedEndpointTransport::Ssh { address }
            | ManagedEndpointTransport::Tls { address, .. } => address.into(),
        };
        let connection = connection_row(screen)
            .child(connection_label(
                screen,
                &entry.config.name,
                &address,
                workspace.and_then(|value| value.verified_engine.as_ref()),
            ))
            .children((!entry.config.labels.is_empty()).then(|| {
                row(locale)
                    .gap_1()
                    .flex_wrap()
                    .children(entry.config.labels.iter().map(|label| {
                        settings_text(
                            screen,
                            Role::Small,
                            technical(label),
                            ColorToken::TextPrimary,
                        )
                        .px_2()
                        .py_1()
                        .rounded_full()
                        .bg(color(screen.palette, ColorToken::AccentSubtle))
                    }))
            }))
            .child(connection_status(screen, workspace.map(Arc::as_ref)));
        let edit = entry.clone();
        let edit_workspace = workspace.cloned();
        let appearance = Appearance::from(screen);
        let test = entry.config.clone();
        let remove = entry.id.clone();
        let actions = row(locale)
            .flex_wrap()
            .gap_2()
            .children(workspace.map(|engine| {
                select_button(
                    screen,
                    SharedString::from(format!("use-environment:{}", entry.id)),
                    engine,
                    cx,
                )
            }))
            .child(
                Button::new(SharedString::from(format!(
                    "test-saved-environment:{}",
                    entry.id
                )))
                .outline()
                .label(gui::GUI_ENVIRONMENT_TEST.text(locale))
                .disabled(disabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let config = test.clone();
                    this.update_environments(
                        async move { Ok(Update::Tested(config.test().await?)) },
                        None,
                        cx,
                    );
                })),
            )
            .child(
                Button::new(SharedString::from(format!("edit-environment:{}", entry.id)))
                    .outline()
                    .label(gui::GUI_ENVIRONMENT_EDIT.text(locale))
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_environment_editor(
                            EditorTarget::Saved(edit.clone(), edit_workspace.clone()),
                            appearance.clone(),
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "remove-environment:{}",
                    entry.id
                )))
                .ghost()
                .label(gui::GUI_DESKTOP_REMOVE.text(locale))
                .disabled(disabled)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.confirm_environment_removal(remove.clone(), locale, window, cx)
                })),
            );
        connection.child(actions)
    }
}

fn connection_row(screen: &ScreenContext<'_>) -> Div {
    row(screen.locale)
        .items_center()
        .gap_3()
        .flex_wrap()
        .py_3()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
}

fn connection_label(
    screen: &ScreenContext<'_>,
    name: &str,
    address: &str,
    verified: Option<&VerifiedEngine>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(140.))
        .flex_1()
        .child(settings_text(
            screen,
            Role::Label,
            technical(name),
            ColorToken::TextPrimary,
        ))
        .child(settings_text(
            screen,
            Role::Small,
            technical(address),
            ColorToken::TextMuted,
        ))
        .children(verified.map(|engine| {
            settings_text(
                screen,
                Role::Small,
                technical(&format!("{} · {}", engine.name, engine.version)),
                ColorToken::TextMuted,
            )
        }))
}

fn connection_status(screen: &ScreenContext<'_>, workspace: Option<&ExternalWorkspace>) -> Div {
    let (label, color) = match workspace {
        Some(workspace) if workspace.reachable => {
            (gui::GUI_ENVIRONMENT_CONNECTED, ColorToken::StatusRunning)
        }
        Some(_) => (gui::GUI_DESKTOP_UNAVAILABLE, ColorToken::TextMuted),
        None => (gui::GUI_ENVIRONMENT_UNCHECKED, ColorToken::TextMuted),
    };
    settings_text(screen, Role::Small, label.text(screen.locale), color)
}

fn select_button(
    screen: &ScreenContext<'_>,
    id: impl Into<marsa_ui::gpui::ElementId>,
    workspace: &ExternalWorkspace,
    cx: &Context<SystemView>,
) -> Button {
    let selection = Environment::from(workspace);
    Button::new(id)
        .outline()
        .label(gui::GUI_ENVIRONMENT_USE.text(screen.locale))
        .disabled(
            workspace.endpoint.managed_id().is_none() && workspace.endpoint.address().is_empty(),
        )
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SelectContext(selection.clone()))))
}

fn feedback_text(screen: &ScreenContext<'_>, feedback: &Feedback) -> Div {
    let (key, token) = match feedback {
        Feedback::Verified(_) | Feedback::Connected => {
            (gui::GUI_ENVIRONMENT_TEST_PASSED, ColorToken::StatusRunning)
        }
        Feedback::Saved => (gui::GUI_ENVIRONMENT_SAVED, ColorToken::StatusRunning),
        Feedback::Removed => (gui::GUI_ENVIRONMENT_REMOVED, ColorToken::TextMuted),
        Feedback::Failed(key) => (*key, ColorToken::StatusWarning),
    };
    let label = settings_text(screen, Role::Small, key.text(screen.locale), token);
    if let Feedback::Verified(engine) = feedback {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(label)
            .child(settings_text(
                screen,
                Role::Mono,
                technical(&format!("{} · {}", engine.name, engine.version)),
                ColorToken::TextMuted,
            ))
    } else {
        label
    }
}

fn error_text(error: &ManagedEndpointError) -> TextKey {
    match error {
        ManagedEndpointError::InvalidName => gui::GUI_ENVIRONMENT_INVALID_NAME,
        ManagedEndpointError::InvalidLabels => gui::GUI_ENVIRONMENT_INVALID_LABELS,
        ManagedEndpointError::InvalidAddress | ManagedEndpointError::LocalMarsa => {
            gui::GUI_ENVIRONMENT_INVALID_ADDRESS
        }
        ManagedEndpointError::InvalidCredentialPath | ManagedEndpointError::Credentials(_) => {
            gui::GUI_ENVIRONMENT_INVALID_CREDENTIALS
        }
        ManagedEndpointError::Connection(_) | ManagedEndpointError::InvalidEngineIdentity => {
            gui::GUI_ENVIRONMENT_TEST_FAILED
        }
        _ => gui::GUI_ENVIRONMENT_STORE_ERROR,
    }
}
