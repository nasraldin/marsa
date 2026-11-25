//! First-run workspace setup through the authenticated owner API.

mod view;

use std::collections::BTreeSet;

use marsa_client::{
    ClientError,
    api::{
        AddShareRequest, EnsureDockerContextRequest, GetSettingsRequest, ListSharesRequest,
        UpdateSettingsRequest,
    },
    request_id,
};
use marsa_ui::{
    Locale,
    gpui::{Context, PathPromptOptions, Task},
    gui_keys as gui,
    preferences::{Language, Settings, Theme},
};

use crate::services::Services;

use super::Desktop;

const SETUP_API_MINOR: u32 = 12;
const MAX_FOLDERS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Language,
    Appearance,
    Sharing,
    Docker,
}

impl Step {
    const ALL: [Self; 4] = [
        Self::Language,
        Self::Appearance,
        Self::Sharing,
        Self::Docker,
    ];

    fn next(self) -> Self {
        Self::ALL[(self as usize + 1).min(Self::ALL.len() - 1)]
    }

    fn back(self) -> Self {
        Self::ALL[(self as usize).saturating_sub(1)]
    }
}

pub(super) struct Setup {
    step: Step,
    locale: Locale,
    appearance: Theme,
    initial_locale: Locale,
    initial_appearance: Theme,
    folders: BTreeSet<String>,
    pending: bool,
    failed: bool,
}

pub(super) fn probe(services: &Services, cx: &Context<Desktop>) -> Task<()> {
    let mut request = services.request(|client| async move {
        if client.api_minor() < SETUP_API_MINOR {
            return Ok(false);
        }
        let snapshot = client
            .settings()
            .get(GetSettingsRequest {})
            .await?
            .into_inner();
        let settings = Settings::from_toml(&snapshot.settings_toml)
            .map_err(|_| ClientError::InvalidConfiguration)?;
        Ok(!settings.ui.setup_complete)
    });
    cx.spawn(async move |desktop, cx| {
        let required = request.result().await;
        let _ = desktop.update(cx, |desktop, cx| {
            desktop.setup_complete = matches!(required, Some(Ok(false)));
            if matches!(required, Some(Ok(true))) {
                desktop.begin_setup(cx);
            }
            cx.notify();
        });
    })
}

impl Desktop {
    pub(super) fn begin_setup(&mut self, cx: &mut Context<Self>) {
        if self.onboarding.is_some() {
            return;
        }
        let appearance = self
            .system
            .read(cx)
            .preferences()
            .map_or(Theme::System, |(_, theme)| theme);
        self.navigate(super::Route::Overview, cx);
        self.onboarding = Some(Setup {
            step: Step::Language,
            locale: self.locale,
            appearance,
            initial_locale: self.locale,
            initial_appearance: self.appearance_preference,
            folders: BTreeSet::new(),
            pending: false,
            failed: false,
        });
        cx.notify();
    }

    pub(super) fn dismiss_setup(&mut self, cx: &mut Context<Self>) {
        if self.onboarding.as_ref().is_none_or(|setup| setup.pending) {
            return;
        }
        if let Some(setup) = self.onboarding.take() {
            self.locale = setup.initial_locale;
            self.appearance_preference = setup.initial_appearance;
            marsa_ui::direction::set_locale(self.locale, cx);
            self.apply_setup_theme(setup.initial_appearance, cx);
            cx.notify();
        }
    }

    fn setup_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if let Some(setup) = &mut self.onboarding {
            setup.locale = locale;
            self.locale = locale;
            marsa_ui::direction::set_locale(locale, cx);
            cx.notify();
        }
    }

    fn setup_appearance(&mut self, appearance: Theme, cx: &mut Context<Self>) {
        if let Some(setup) = &mut self.onboarding {
            setup.appearance = appearance;
            self.appearance_preference = appearance;
            self.apply_setup_theme(appearance, cx);
            cx.notify();
        }
    }

    fn apply_setup_theme(&mut self, appearance: Theme, cx: &mut Context<Self>) {
        use marsa_ui::gpui_component::{Theme as ComponentTheme, ThemeMode};
        self.themed_palette = None;
        match appearance {
            Theme::System => ComponentTheme::sync_system_appearance(None, cx),
            Theme::Light => ComponentTheme::change(ThemeMode::Light, None, cx),
            Theme::Dark => ComponentTheme::change(ThemeMode::Dark, None, cx),
        }
    }

    fn choose_setup_folder(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = &mut self.onboarding else {
            return;
        };
        if setup.pending || setup.folders.len() >= MAX_FOLDERS {
            return;
        }
        setup.pending = true;
        let dialog = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                gui::GUI_DESKTOP_CHOOSE_A_FOLDER_TO_SHARE
                    .text(self.locale)
                    .into(),
            ),
        });
        self.setup_task = Some(cx.spawn(async move |desktop, cx| {
            let result = dialog.await;
            let _ = desktop.update(cx, |desktop, cx| {
                if desktop.onboarding.is_none() {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(setup) = &mut desktop.onboarding {
                            setup.pending = false;
                        }
                        desktop.add_setup_folders(&paths, cx);
                    }
                    Ok(Ok(None)) => {
                        if let Some(setup) = &mut desktop.onboarding {
                            setup.pending = false;
                        }
                        cx.notify();
                    }
                    _ => {
                        if let Some(setup) = &mut desktop.onboarding {
                            setup.pending = false;
                            setup.failed = true;
                        }
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }

    fn add_setup_folders(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let Some(setup) = &mut self.onboarding else {
            return;
        };
        if setup.pending {
            return;
        }
        let mut added = false;
        for path in paths {
            if setup.folders.len() >= MAX_FOLDERS {
                break;
            }
            if !path.is_dir() {
                continue;
            }
            let Some(value) = path.to_str() else {
                setup.failed = true;
                continue;
            };
            added |= setup.folders.insert(value.to_owned());
        }
        if added {
            setup.failed = false;
        }
        cx.notify();
    }

    fn setup_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(setup) = &mut self.onboarding else {
            return;
        };
        if setup.pending {
            return;
        }
        if forward && setup.step == Step::Docker {
            self.finish_setup(cx);
            return;
        }
        setup.failed = false;
        setup.step = if forward {
            setup.step.next()
        } else {
            setup.step.back()
        };
        cx.notify();
    }

    fn finish_setup(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = &mut self.onboarding else {
            return;
        };
        if setup.pending {
            return;
        }
        let Some(services) = &self.services else {
            setup.failed = true;
            cx.notify();
            return;
        };
        let folders = setup.folders.clone();
        let language = match setup.locale {
            Locale::EnglishUs => Language::English,
            Locale::Arabic => Language::Arabic,
        };
        let appearance = setup.appearance;
        setup.pending = true;
        setup.failed = false;
        let mut response = services.request(move |client| async move {
            client
                .docker_discovery()
                .ensure_context(EnsureDockerContextRequest {
                    request_id: new_request_id()?,
                })
                .await?;
            if !folders.is_empty() {
                let mut shares = client.shares();
                let existing = shares
                    .list(ListSharesRequest {})
                    .await?
                    .into_inner()
                    .shares
                    .into_iter()
                    .map(|share| share.host_path)
                    .collect::<BTreeSet<_>>();
                for path in folders.difference(&existing) {
                    // Daemon share grants are read/write only (read_only is unimplemented).
                    shares
                        .add(AddShareRequest {
                            request_id: new_request_id()?,
                            host_path: path.clone(),
                            read_only: false,
                            approval_id: None,
                            persist: true,
                        })
                        .await?;
                }
            }
            let mut preferences = client.settings();
            let snapshot = preferences.get(GetSettingsRequest {}).await?.into_inner();
            let mut settings = Settings::from_toml(&snapshot.settings_toml)
                .map_err(|_| ClientError::InvalidConfiguration)?;
            settings.ui.language = language;
            settings.ui.theme = appearance;
            if client.api_minor() >= SETUP_API_MINOR {
                settings.ui.setup_complete = true;
            }
            let settings_toml = settings
                .to_toml()
                .map_err(|_| ClientError::InvalidConfiguration)?;
            preferences
                .update(UpdateSettingsRequest {
                    request_id: new_request_id()?,
                    settings_toml,
                    expected_revision: snapshot.revision,
                })
                .await?;
            Ok(())
        });
        self.setup_task = Some(cx.spawn(async move |desktop, cx| {
            let result = response.result().await;
            let _ = desktop.update(cx, |desktop, cx| {
                if matches!(result, Some(Ok(()))) {
                    desktop.setup_complete = true;
                    desktop.onboarding = None;
                    desktop.navigate(super::Route::Overview, cx);
                } else if let Some(setup) = &mut desktop.onboarding {
                    setup.pending = false;
                    setup.failed = true;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
}

fn new_request_id() -> Result<String, ClientError> {
    request_id().map_err(|_| ClientError::InvalidConfiguration)
}
