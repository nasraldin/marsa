//! Configure and run an explicitly requested scan on the engine open in the editor.

use std::{rc::Rc, sync::Arc};

use marsa_client::docker::{
    DockerClient, ImageId, ImageSummary,
    scanning::{ScanError, ScanSettings, Scanner, ScannerChoice, ScannerResult, Severity},
};
use marsa_ui::{
    ColorToken, Locale, TextKey,
    gpui::{Context, Div, ParentElement as _, SharedString, Styled as _, Task, Window, div},
    gpui_component::{
        Disableable as _, IconName, Selectable as _,
        button::{Button, ButtonVariants as _},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{
    SystemView,
    editor::{Editor, EditorTarget},
    settings_text,
};
use crate::screens::common::{ScreenContext, row, technical};

#[derive(Default)]
pub(super) struct Security {
    token: Rc<()>,
    settings: Option<ScanSettings>,
    images: Option<Arc<[ImageChoice]>>,
    selection: Option<ImageChoice>,
    reports: Vec<ScannerResult>,
    feedback: Option<TextKey>,
    task: Option<Task<()>>,
    scanning: bool,
}

#[derive(Clone)]
struct ImageChoice {
    id: ImageId,
    label: SharedString,
}

enum Work {
    Load,
    Save(ScanSettings),
    Images,
    Scan(ImageId),
}

enum Change {
    Settings(ScanSettings),
    Images(Arc<[ImageChoice]>),
    Reports(Vec<ScannerResult>),
    Failed(TextKey),
}

impl Work {
    async fn run(self, docker: DockerClient) -> Change {
        match self {
            Self::Load => match docker.scan_settings().await {
                Ok(settings) => Change::Settings(settings),
                Err(error) => Change::Failed(error_text(&error)),
            },
            Self::Save(settings) => match docker.save_scan_settings(settings).await {
                Ok(()) => Change::Settings(settings),
                Err(error) => Change::Failed(error_text(&error)),
            },
            Self::Images => match docker.images().await {
                Ok(images) => Change::Images(image_choices(images).into()),
                Err(_) => Change::Failed(gui::GUI_ENVIRONMENT_SCAN_IMAGES_FAILED),
            },
            Self::Scan(image) => match docker.scan_image(&image).await {
                Ok(reports) => Change::Reports(reports),
                Err(error) => Change::Failed(error_text(&error)),
            },
        }
    }
}

impl SystemView {
    pub(super) fn load_environment_security(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_environment_security(Work::Load, window, cx);
    }

    fn run_environment_security(
        &mut self,
        work: Work,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(editor), Some(services)) = (
            self.environment_settings.editor.as_ref(),
            self.services.as_ref(),
        ) else {
            return;
        };
        if editor.security.task.is_some() {
            return;
        }
        let Some(target) = editor
            .workspace(cx)
            .map(|workspace| Some(workspace.endpoint.clone()))
            .or_else(|| matches!(editor.target, EditorTarget::Builtin).then_some(None))
        else {
            return;
        };
        let docker = services.resource_docker(target);
        let token = editor.security.token.clone();
        let scanning = matches!(work, Work::Scan(_));
        let mut pending = services.pending(async move { Ok(work.run(docker).await) });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = pending.result().await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(editor) = this.environment_settings.editor.as_mut() else {
                        return;
                    };
                    if !Rc::ptr_eq(&editor.security.token, &token) {
                        return;
                    }
                    let panel = &mut editor.security;
                    panel.task = None;
                    panel.scanning = false;
                    match result {
                        Some(Ok(Change::Settings(settings))) => {
                            if panel.settings != Some(settings) {
                                panel.reports.clear();
                            }
                            panel.settings = Some(settings);
                        }
                        Some(Ok(Change::Images(images))) => {
                            if panel.selection.as_ref().is_some_and(|selected| {
                                !images.iter().any(|image| image.id == selected.id)
                            }) {
                                panel.selection = None;
                                panel.reports.clear();
                            }
                            panel.images = Some(images);
                        }
                        Some(Ok(Change::Reports(reports))) => panel.reports = reports,
                        Some(Ok(Change::Failed(key))) => panel.feedback = Some(key),
                        _ => panel.feedback = Some(gui::GUI_ENVIRONMENT_SCAN_FAILED),
                    }
                    cx.notify();
                });
                window.refresh();
            });
        });
        if let Some(editor) = self.environment_settings.editor.as_mut() {
            editor.security.task = Some(task);
            editor.security.feedback = None;
            editor.security.scanning = scanning;
            if scanning {
                editor.security.reports.clear();
            }
        }
        cx.notify();
    }

    pub(super) fn environment_security(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let panel = &editor.security;
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_text(
                screen,
                Role::Body,
                gui::GUI_ENVIRONMENT_SCAN_TITLE.text(locale),
                ColorToken::TextPrimary,
            ))
            .child(message(screen, gui::GUI_ENVIRONMENT_SCAN_HELP, false));

        if editor.workspace(cx).is_none() && !matches!(editor.target, EditorTarget::Builtin) {
            return body.child(message(screen, gui::GUI_ENVIRONMENT_OFFLINE, true));
        }

        let busy = panel.task.is_some();
        if let Some(settings) = panel.settings {
            let toggle = if settings.enabled {
                gui::GUI_ENVIRONMENT_SCAN_DISABLE
            } else {
                gui::GUI_ENVIRONMENT_SCAN_ENABLE
            };
            body = body.child(
                row(locale)
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("environment-scan-enabled")
                            .outline()
                            .label(toggle.text(locale))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_environment_security(
                                    Work::Save(ScanSettings {
                                        enabled: !settings.enabled,
                                        ..settings
                                    }),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .children(
                        [
                            ScannerChoice::Grype,
                            ScannerChoice::Trivy,
                            ScannerChoice::Both,
                        ]
                        .into_iter()
                        .map(|choice| {
                            Button::new(("environment-scan-choice", choice as usize))
                                .outline()
                                .selected(choice == settings.choice)
                                .disabled(busy)
                                .label(choice_label(choice, locale))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_environment_security(
                                        Work::Save(ScanSettings { choice, ..settings }),
                                        window,
                                        cx,
                                    );
                                }))
                        }),
                    ),
            );
            if settings.enabled {
                body = body
                    .child(self.scan_image_controls(panel, screen, cx))
                    .children(
                        panel
                            .reports
                            .iter()
                            .map(|result| report_view(screen, result)),
                    );
            }
        } else if !busy {
            body = body.child(
                Button::new("environment-scan-retry-settings")
                    .outline()
                    .label(gui::GUI_DESKTOP_REFRESH.text(locale))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.load_environment_security(window, cx);
                    })),
            );
        }

        if busy {
            let key = if panel.scanning {
                gui::GUI_ENVIRONMENT_SCAN_RUNNING
            } else {
                gui::GUI_DESKTOP_WORKING
            };
            body = body.child(message(screen, key, false));
        }
        body.children(panel.feedback.map(|key| message(screen, key, true)))
    }

    fn scan_image_controls(
        &self,
        panel: &Security,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let busy = panel.task.is_some();
        let mut controls = row(locale).flex_wrap().gap_2().child(
            Button::new("environment-scan-load-images")
                .outline()
                .label(if panel.images.is_some() {
                    gui::GUI_DESKTOP_REFRESH.text(locale)
                } else {
                    gui::GUI_ENVIRONMENT_SCAN_LOAD.text(locale)
                })
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.run_environment_security(Work::Images, window, cx);
                })),
        );
        let Some(images) = panel.images.clone() else {
            return controls;
        };
        if images.is_empty() {
            return controls.child(message(screen, gui::GUI_ENVIRONMENT_SCAN_NO_IMAGES, false));
        }
        let label = panel.selection.as_ref().map_or_else(
            || gui::GUI_DESKTOP_CONTAINERS_CHOOSE_IMAGE.text(locale).into(),
            |image| image.label.clone(),
        );
        let owner = cx.weak_entity();
        let id = panel.selection.as_ref().map(|image| image.id.clone());
        controls = controls.child(
            Button::new("environment-scan-image")
                .outline()
                .icon(IconName::ChevronDown)
                .label(label)
                .disabled(busy)
                .dropdown_menu(move |menu, _, _| {
                    images.iter().fold(menu, |menu, image| {
                        let selected = id.as_ref() == Some(&image.id);
                        let image = image.clone();
                        let owner = owner.clone();
                        menu.item(
                            PopupMenuItem::new(image.label.clone())
                                .checked(selected)
                                .on_click(move |_, window, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        if let Some(editor) = &mut this.environment_settings.editor
                                            && editor.security.task.is_none()
                                        {
                                            editor.security.selection = Some(image.clone());
                                            editor.security.reports.clear();
                                            editor.security.feedback = None;
                                            cx.notify();
                                        }
                                    });
                                    window.refresh();
                                }),
                        )
                    })
                }),
        );
        if let Some(image) = &panel.selection {
            let image = image.id.clone();
            controls = controls.child(
                Button::new("environment-scan-run")
                    .primary()
                    .label(gui::GUI_ENVIRONMENT_SCAN_RUN.text(locale))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_environment_security(Work::Scan(image.clone()), window, cx);
                    })),
            );
        }
        controls
    }
}

fn image_choices(images: Vec<ImageSummary>) -> Vec<ImageChoice> {
    let mut choices = Vec::new();
    for image in images {
        let Ok(id) = ImageId::parse(image.id) else {
            continue;
        };
        let digest = &id.as_str()[7..19];
        let tag = image.repo_tags.iter().find(|tag| {
            tag.as_str() != "<none>:<none>"
                && !tag.is_empty()
                && tag.bytes().all(|byte| byte.is_ascii_graphic())
        });
        let label = tag.map_or_else(
            || digest.to_owned(),
            |tag| format!("{} · {digest}", tag.chars().take(80).collect::<String>()),
        );
        choices.push(ImageChoice {
            id,
            label: technical(&label),
        });
    }
    choices.sort_unstable_by(|a, b| a.label.cmp(&b.label));
    choices
}

fn report_view(screen: &ScreenContext<'_>, result: &ScannerResult) -> Div {
    let (scanner, report) = result;
    let locale = screen.locale;
    let body = div().flex().flex_col().gap_1().child(settings_text(
        screen,
        Role::Body,
        scanner.name(),
        ColorToken::TextPrimary,
    ));
    match report {
        Ok(report) if report.total == 0 => body.child(message(
            screen,
            gui::GUI_ENVIRONMENT_SCAN_NO_FINDINGS,
            false,
        )),
        Ok(report) => body
            .child(settings_text(
                screen,
                Role::Small,
                format!(
                    "{} · {}",
                    gui::GUI_ENVIRONMENT_SCAN_FINDINGS.text(locale),
                    report.total
                ),
                ColorToken::TextMuted,
            ))
            .children(
                (report.total > report.findings.len())
                    .then(|| message(screen, gui::GUI_ENVIRONMENT_SCAN_SHOWN, false)),
            )
            .children(report.findings.iter().map(|finding| {
                let (severity, severity_color) = match finding.severity {
                    Severity::Critical => {
                        (gui::GUI_ENVIRONMENT_SCAN_CRITICAL, ColorToken::StatusError)
                    }
                    Severity::High => (gui::GUI_ENVIRONMENT_SCAN_HIGH, ColorToken::StatusError),
                    Severity::Medium => {
                        (gui::GUI_ENVIRONMENT_SCAN_MEDIUM, ColorToken::StatusWarning)
                    }
                    Severity::Low => (gui::GUI_ENVIRONMENT_SCAN_LOW, ColorToken::TextMuted),
                    Severity::Negligible => {
                        (gui::GUI_ENVIRONMENT_SCAN_NEGLIGIBLE, ColorToken::TextMuted)
                    }
                    Severity::Unknown => (gui::GUI_ENVIRONMENT_SCAN_UNKNOWN, ColorToken::TextMuted),
                };
                row(locale)
                    .flex_wrap()
                    .gap_2()
                    .child(settings_text(
                        screen,
                        Role::Small,
                        severity.text(locale),
                        severity_color,
                    ))
                    .child(settings_text(
                        screen,
                        Role::Mono,
                        technical(&format!(
                            "{} · {} {}",
                            finding.id, finding.package, finding.installed
                        )),
                        ColorToken::TextPrimary,
                    ))
                    .children((!finding.fix.is_empty()).then(|| {
                        row(locale)
                            .gap_1()
                            .child(message(screen, gui::GUI_ENVIRONMENT_SCAN_FIX, false))
                            .child(settings_text(
                                screen,
                                Role::Mono,
                                technical(&finding.fix),
                                ColorToken::TextMuted,
                            ))
                    }))
            })),
        Err(error) => body
            .child(message(screen, error_text(error), true))
            .children(matches!(error, ScanError::Missing).then(|| {
                let scanner = *scanner;
                Button::new(("environment-scan-install", scanner as usize))
                    .outline()
                    .label(gui::GUI_ENVIRONMENT_SCAN_INSTALL.text(locale))
                    .on_click(move |_, _, cx| {
                        cx.open_url(match scanner {
                            Scanner::Grype => "https://oss.anchore.com/docs/installation/grype/",
                            Scanner::Trivy => {
                                "https://trivy.dev/docs/latest/getting-started/installation/"
                            }
                        });
                    })
            })),
    }
}

fn choice_label(choice: ScannerChoice, locale: Locale) -> &'static str {
    match choice {
        ScannerChoice::Grype => Scanner::Grype.name(),
        ScannerChoice::Trivy => Scanner::Trivy.name(),
        ScannerChoice::Both => gui::GUI_ENVIRONMENT_SCAN_BOTH.text(locale),
    }
}

fn error_text(error: &ScanError) -> TextKey {
    match error {
        ScanError::Endpoint => gui::GUI_ENVIRONMENT_OFFLINE,
        ScanError::State(_) => gui::GUI_ENVIRONMENT_STORE_ERROR,
        ScanError::Missing => gui::GUI_ENVIRONMENT_SCAN_MISSING,
        ScanError::Export(_) => gui::GUI_ENVIRONMENT_SCAN_EXPORT_FAILED,
        ScanError::Timeout => gui::GUI_ENVIRONMENT_SCAN_TIMEOUT,
        ScanError::Report => gui::GUI_ENVIRONMENT_SCAN_INVALID_REPORT,
        _ => gui::GUI_ENVIRONMENT_SCAN_FAILED,
    }
}

fn message(screen: &ScreenContext<'_>, key: TextKey, error: bool) -> Div {
    settings_text(
        screen,
        Role::Small,
        key.text(screen.locale),
        if error {
            ColorToken::StatusWarning
        } else {
            ColorToken::TextMuted
        },
    )
}
