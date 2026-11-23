//! Manage Docker connections and their environment settings.

mod editor;
mod observations;
mod table;
mod vm;

use std::{collections::HashMap, future::Future, sync::Arc};

use editor::{Editor, EditorTarget};
use table::{ConnectionCheck, ConnectionKey, check_builtin_connection};

use marsa_client::docker::{
    ManagedEndpoint, ManagedEndpointError, ManagedEndpointStore, VerifiedEngine,
};
use marsa_ui::{
    ColorToken, Locale, TextKey,
    gpui::{
        Context, Div, ParentElement as _, PromptButton, PromptLevel, Styled as _, Task, Window, div,
    },
    gpui_component::WindowExt as _,
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, settings_text};
use crate::screens::common::{ScreenContext, technical};

#[derive(Default)]
pub(in crate::screens::system) struct EnvironmentSettings {
    saved: Option<Vec<ManagedEndpoint>>,
    editor: Option<Editor>,
    task: Option<Task<()>>,
    feedback: Option<Feedback>,
    checks: HashMap<ConnectionKey, ConnectionCheck>,
    checking: bool,
}

enum Feedback {
    Verified(VerifiedEngine),
    Connected,
    Checked { passed: usize, total: usize },
    Saved,
    Removed,
    Failed(TextKey),
}

enum Update {
    Listed(Vec<ManagedEndpoint>),
    Tested(VerifiedEngine),
    Checked(Vec<(ConnectionKey, ConnectionCheck)>),
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
        self.environment_settings.checks.clear();
        self.refresh_external_inventory(cx);
        self.load_component_versions(cx);
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
                panel.checking = false;
                match result {
                    Some(Ok(Ok(Update::Listed(saved)))) => panel.saved = Some(saved),
                    Some(Ok(Ok(Update::Tested(engine)))) => {
                        panel.feedback = Some(Feedback::Verified(engine))
                    }
                    Some(Ok(Ok(Update::Checked(checks)))) => {
                        panel.feedback = Some(Feedback::Checked {
                            passed: checks.iter().filter(|(_, check)| check.passed()).count(),
                            total: checks.len(),
                        });
                        panel.checks.extend(checks);
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
                        panel.checks.clear();
                        panel.feedback = Some(feedback);
                        this.external_inventory_task = None;
                        this.refresh_external_inventory(cx);
                    }
                    Some(Ok(Ok(Update::Closed(saved)))) => {
                        panel.editor = None;
                        panel.saved = Some(saved);
                        panel.checks.clear();
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
                let mut check = check_builtin_connection(services);
                self.update_environments(
                    async move {
                        let connected =
                            matches!(check.result().await, Some(Ok(report)) if report.passed());
                        Ok(Update::MarsaTest(connected))
                    },
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
}

fn feedback_text(screen: &ScreenContext<'_>, feedback: &Feedback) -> Div {
    let (key, token) = match feedback {
        Feedback::Verified(_) | Feedback::Connected => {
            (gui::GUI_ENVIRONMENT_TEST_PASSED, ColorToken::StatusRunning)
        }
        Feedback::Checked { passed, total } => {
            return settings_text(
                screen,
                Role::Small,
                format!(
                    "{}  {passed}/{total}",
                    gui::GUI_ENVIRONMENT_TEST_RESULTS.text(screen.locale)
                ),
                if passed == total {
                    ColorToken::StatusRunning
                } else {
                    ColorToken::StatusWarning
                },
            );
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
