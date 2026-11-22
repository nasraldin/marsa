//! Safe confirmations and inputs for resource actions.

use crate::screens::common::message;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{App, AppContext, Context, Div, Entity, ParentElement, Styled, Window, div, px},
    gpui_component::{
        Disableable, WindowExt,
        button::{Button, ButtonVariants},
        checkbox::Checkbox,
        input::{Input, InputState},
    },
    tokens::{scale::Space, typography::Role},
};

use crate::screens::common::ScreenContext;

use super::{ResourceKind, ResourcesView, logical_row, text, tone};

#[derive(Clone)]
pub(super) enum FormKind {
    Pull,
    Run {
        id: String,
        reference: String,
    },
    RemoveImage {
        id: String,
        reference: String,
        users: Vec<String>,
    },
    CreateVolume,
    RemoveVolume {
        name: String,
        users: Vec<String>,
        in_use: bool,
    },
    CreateNetwork,
    RemoveNetwork {
        id: String,
        name: String,
        users: Vec<String>,
    },
    ComposeUp {
        project: String,
    },
    ComposeRestart {
        project: String,
    },
    ComposeDown {
        project: String,
        volumes: bool,
    },
}

impl FormKind {
    pub(super) fn page(&self) -> ResourceKind {
        match self {
            Self::Pull | Self::Run { .. } | Self::RemoveImage { .. } => ResourceKind::Images,
            Self::CreateVolume | Self::RemoveVolume { .. } => ResourceKind::Volumes,
            Self::CreateNetwork | Self::RemoveNetwork { .. } => ResourceKind::Networks,
            Self::ComposeDown { volumes: true, .. } => ResourceKind::Compose,
        }
    }

    pub(super) fn title(&self, locale: Locale) -> &'static str {
        match self {
            Self::Pull => gui::GUI_DESKTOP_PULL_IMAGE.text(locale),
            Self::Run { .. } => gui::GUI_DESKTOP_RUN_IMAGE.text(locale),
            Self::RemoveImage { .. } => gui::GUI_DESKTOP_REMOVE_IMAGE.text(locale),
            Self::CreateVolume => gui::GUI_DESKTOP_CREATE_VOLUME.text(locale),
            Self::RemoveVolume { .. } => gui::GUI_DESKTOP_REMOVE_VOLUME.text(locale),
            Self::CreateNetwork => gui::GUI_DESKTOP_CREATE_NETWORK.text(locale),
            Self::RemoveNetwork { .. } => gui::GUI_DESKTOP_REMOVE_NETWORK.text(locale),
            Self::ComposeUp { .. } => gui::GUI_DESKTOP_START_PROJECT.text(locale),
            Self::ComposeRestart { .. } => gui::GUI_DESKTOP_RESTART_PROJECT.text(locale),
            Self::ComposeDown { volumes: false, .. } => {
                gui::GUI_DESKTOP_STOP_AND_REMOVE_PROJECT.text(locale)
            }
            Self::ComposeDown { volumes: true, .. } => {
                gui::GUI_DESKTOP_REMOVE_PROJECT_AND_VOLUMES.text(locale)
            }
        }
    }

    fn target(&self) -> Option<&str> {
        match self {
            Self::Run { reference, .. } | Self::RemoveImage { reference, .. } => Some(reference),
            Self::RemoveVolume { name, .. } | Self::RemoveNetwork { name, .. } => Some(name),
            Self::ComposeUp { project }
            | Self::ComposeRestart { project }
            | Self::ComposeDown { project, .. } => Some(project),
            Self::Pull | Self::CreateVolume | Self::CreateNetwork => None,
        }
    }

    fn placeholder(&self, locale: Locale) -> Option<&'static str> {
        match self {
            Self::Pull => Some(gui::GUI_DESKTOP_IMAGE_REFERENCE.text(locale)),
            Self::Run { .. } => Some(gui::GUI_DESKTOP_CONTAINER_NAME_OPTIONAL.text(locale)),
            Self::CreateNetwork => Some(gui::GUI_DESKTOP_NETWORK_NAME.text(locale)),
            Self::CreateVolume => Some(gui::GUI_DESKTOP_VOLUME_NAME.text(locale)),
            Self::RemoveVolume { .. } => {
                Some(gui::GUI_DESKTOP_TYPE_THE_VOLUME_NAME_TO_CONFIRM.text(locale))
            }
            Self::ComposeDown { volumes: true, .. } => {
                Some(gui::GUI_DESKTOP_TYPE_THE_PROJECT_NAME_TO_CONFIRM.text(locale))
            }
            _ => None,
        }
    }

    fn users(&self) -> &[String] {
        match self {
            Self::RemoveImage { users, .. }
            | Self::RemoveVolume { users, .. }
            | Self::RemoveNetwork { users, .. } => users,
            _ => &[],
        }
    }

    fn destructive(&self) -> bool {
        matches!(
            self,
            Self::RemoveImage { .. }
                | Self::RemoveVolume { .. }
                | Self::RemoveNetwork { .. }
                | Self::ComposeDown { .. }
        )
    }
}

/// An operation emitted only after its applicable confirmation has passed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResourceRequest {
    Pull(String),
    Run {
        id: String,
        name: Option<String>,
    },
    RemoveImage {
        id: String,
        force: bool,
    },
    CreateVolume(String),
    RemoveVolume {
        name: String,
        typed: String,
    },
    CreateNetwork(String),
    RemoveNetwork(String),
    ComposeUp(String),
    ComposeRestart(String),
    ComposeDown {
        project: String,
        volumes: bool,
        typed: String,
    },
}

pub(super) struct Form {
    pub kind: FormKind,
    input: Option<Entity<InputState>>,
    force: bool,
}

impl Form {
    pub(super) fn new(
        kind: FormKind,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<ResourcesView>,
    ) -> Self {
        let input = kind.placeholder(locale).map(|placeholder| {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            cx.subscribe(&input, |_, _, _, cx| cx.refresh_windows())
                .detach();
            input
        });
        Self {
            kind,
            input,
            force: false,
        }
    }

    fn request(&self, cx: &App) -> Option<ResourceRequest> {
        let value = self
            .input
            .as_ref()
            .map(|input| input.read(cx).value())
            .unwrap_or_default();
        let trimmed = value.trim();
        let nonempty = || (!trimmed.is_empty()).then_some(trimmed);
        Some(match &self.kind {
            FormKind::Pull => ResourceRequest::Pull(nonempty()?.to_owned()),
            FormKind::Run { id, .. } => ResourceRequest::Run {
                id: id.clone(),
                name: nonempty().map(str::to_owned),
            },
            FormKind::RemoveImage { id, users, .. } if users.is_empty() || self.force => {
                ResourceRequest::RemoveImage {
                    id: id.clone(),
                    force: self.force,
                }
            }
            FormKind::CreateVolume => ResourceRequest::CreateVolume(nonempty()?.to_owned()),
            FormKind::RemoveVolume {
                name,
                users,
                in_use,
            } if !in_use && users.is_empty() && value.as_ref() == name.as_str() => {
                ResourceRequest::RemoveVolume {
                    name: name.clone(),
                    typed: value.to_string(),
                }
            }
            FormKind::CreateNetwork => ResourceRequest::CreateNetwork(nonempty()?.to_owned()),
            FormKind::RemoveNetwork { id, users, .. } if users.is_empty() => {
                ResourceRequest::RemoveNetwork(id.clone())
            }
            FormKind::ComposeUp { project } => ResourceRequest::ComposeUp(project.clone()),
            FormKind::ComposeRestart { project } => {
                ResourceRequest::ComposeRestart(project.clone())
            }
            FormKind::ComposeDown { project, volumes }
                if !volumes || value.as_ref() == project.as_str() =>
            {
                ResourceRequest::ComposeDown {
                    project: project.clone(),
                    volumes: *volumes,
                    typed: value.to_string(),
                }
            }
            _ => return None,
        })
    }

    pub(super) fn render(
        &self,
        screen: &ScreenContext<'_>,
        view: &Entity<ResourcesView>,
        cx: &App,
    ) -> Div {
        let title = self.kind.title(screen.locale);
        let mut form = div().flex().flex_col().gap(px(Space::S12.pixels()));
        if let Some(target) = self.kind.target() {
            form = form.child(text(screen, Role::Mono, target.to_owned()));
        }
        if !self.kind.users().is_empty() {
            form = form.child(text(
                screen,
                Role::Body,
                message(screen.locale, gui::GUI_DESKTOP_USED_BY_THESE_CONTAINERS),
            ));
            form = form.children(
                self.kind
                    .users()
                    .iter()
                    .map(|name| text(screen, Role::Mono, name.clone())),
            );
        }
        if matches!(self.kind, FormKind::RemoveVolume { in_use: true, .. })
            || (matches!(
                self.kind,
                FormKind::RemoveVolume { .. } | FormKind::RemoveNetwork { .. }
            ) && !self.kind.users().is_empty())
        {
            form = form.child(
                text(
                    screen,
                    Role::Small,
                    message(screen.locale, gui::GUI_DESKTOP_DISCONNECT_OR_REMOVE_THE_CONTAINERS_BEFORE_REMOVING_THIS_RESOU),
                )
                .text_color(tone(screen, ColorToken::StatusWarning)),
            );
        }
        if matches!(self.kind, FormKind::RemoveImage { .. }) && !self.kind.users().is_empty() {
            let view = view.clone();
            form = form.child(
                Checkbox::new("force-image-remove")
                    .label(message(
                        screen.locale,
                        gui::GUI_DESKTOP_FORCE_REMOVAL_OF_THIS_IN_USE_IMAGE_AND_ALL_ITS_TAGS,
                    ))
                    .checked(self.force)
                    .on_click(move |checked, _, cx| {
                        view.update(cx, |view, cx| {
                            if let Some(form) = &mut view.form {
                                form.force = *checked;
                                cx.refresh_windows();
                            }
                        });
                    }),
            );
        }
        if self.kind.page() == ResourceKind::Compose {
            form = form.child(text(
                screen,
                Role::Body,
                message(
                    screen.locale,
                    gui::GUI_DESKTOP_CHOOSE_THE_COMPOSE_FILE_FOR_THIS_PROJECT_AFTER_CONFIRMING,
                ),
            ));
        }
        if let Some(input) = &self.input {
            let mut control = Input::new(input);
            if let Some(placeholder) = self.kind.placeholder(screen.locale) {
                control = control.aria_label(placeholder);
            }
            form = form.child(control);
        }
        let cancel = view.clone();
        let submit = view.clone();
        let service = screen.services.cloned();
        let locale = screen.locale;
        let request = self.request(cx);
        let submit_button = Button::new("submit-resource-action")
            .label(title)
            .disabled(request.is_none() || service.is_none() || view.read(cx).busy.is_some());
        let submit_button = if self.kind.destructive() {
            submit_button.danger()
        } else {
            submit_button.primary()
        };
        form.child(
            logical_row(screen.locale)
                .justify_end()
                .gap(px(Space::S8.pixels()))
                .child(
                    Button::new("cancel-resource-action")
                        .label(message(screen.locale, gui::GUI_DESKTOP_CANCEL))
                        .on_click(move |_, window, cx| {
                            cancel.update(cx, |view, cx| {
                                view.form = None;
                                cx.refresh_windows();
                            });
                            window.close_dialog(cx);
                        }),
                )
                .child(submit_button.on_click(move |_, window, cx| {
                    let Some(service) = &service else { return };
                    let request = submit.update(cx, |view, cx| {
                        let form = view.form.as_ref()?;
                        let request = (form.kind.page(), form.request(cx)?);
                        view.form = None;
                        Some(request)
                    });
                    if let Some((kind, request)) = request {
                        window.close_dialog(cx);
                        submit.update(cx, |view, cx| {
                            view.dispatch(kind, request, service.clone(), locale, cx)
                        });
                    }
                })),
        )
    }
}
