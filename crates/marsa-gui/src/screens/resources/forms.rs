//! Safe confirmations and inputs for resource actions.

use crate::screens::{
    common::{ScreenContext, dialog_actions, message},
    external_inventory::dialog_scope,
};
use marsa_client::compose::valid_project_name;
use marsa_ui::gui_keys as gui;
use marsa_ui::{
    ColorToken, Locale,
    gpui::{
        App, AppContext, Context, Div, Entity, FontWeight, ParentElement, Styled, Window, div, px,
    },
    gpui_component::{
        Disableable, WindowExt,
        button::{Button, ButtonVariants},
        checkbox::Checkbox,
        input::{Editor, EditorState, InputEvent, InputState},
    },
    preferences::valid_compose_template,
    tokens::{
        scale::{Radius, Space},
        typography::Role,
    },
};

use super::{ResourceKind, ResourcesView, text, tone};

#[derive(Clone)]
pub(crate) enum FormKind {
    Pull,
    ImportImage,
    PushImage {
        id: String,
        reference: String,
    },
    ExportImage {
        id: String,
        reference: String,
    },
    NetworkMembership {
        id: String,
        name: String,
        attach: bool,
    },
    CreateStack {
        template: String,
    },
    DeployStack,
    Run {
        id: String,
        reference: String,
    },
    TagImage {
        id: String,
        reference: String,
    },
    RemoveImage {
        id: String,
        reference: String,
        users: Vec<String>,
    },
    RemoveImages {
        items: Vec<(String, String, Vec<String>)>,
    },
    /// Bulk prune via Docker `/images/prune`. `dangling` selects untagged layers only.
    PruneImages {
        dangling: bool,
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
            Self::ImportImage
            | Self::PushImage { .. }
            | Self::ExportImage { .. }
            | Self::Pull
            | Self::Run { .. }
            | Self::RemoveImage { .. }
            | Self::RemoveImages { .. }
            | Self::PruneImages { .. }
            | Self::TagImage { .. } => ResourceKind::Images,
            Self::NetworkMembership { .. } => ResourceKind::Networks,
            Self::CreateStack { .. } | Self::DeployStack => ResourceKind::Compose,
            Self::CreateVolume | Self::RemoveVolume { .. } => ResourceKind::Volumes,
            Self::CreateNetwork | Self::RemoveNetwork { .. } => ResourceKind::Networks,
            Self::ComposeUp { .. } | Self::ComposeRestart { .. } | Self::ComposeDown { .. } => {
                ResourceKind::Compose
            }
        }
    }

    pub(super) fn title(&self, locale: Locale) -> &'static str {
        match self {
            Self::ImportImage => gui::GUI_DESKTOP_IMPORT_IMAGE.text(locale),
            Self::PushImage { .. } => gui::GUI_DESKTOP_PUSH_IMAGE.text(locale),
            Self::ExportImage { .. } => gui::GUI_DESKTOP_EXPORT_IMAGE.text(locale),
            Self::NetworkMembership { attach: true, .. } => {
                gui::GUI_DESKTOP_ATTACH_CONTAINER.text(locale)
            }
            Self::NetworkMembership { attach: false, .. } => {
                gui::GUI_DESKTOP_DETACH_CONTAINER.text(locale)
            }
            Self::CreateStack { .. } => gui::GUI_COMPOSE_NEW_STACK.text(locale),
            Self::DeployStack => gui::GUI_DESKTOP_DEPLOY_STACK.text(locale),
            Self::Pull => gui::GUI_DESKTOP_PULL_IMAGE.text(locale),
            Self::Run { .. } => gui::GUI_DESKTOP_RUN_IMAGE.text(locale),
            Self::TagImage { .. } => gui::GUI_DESKTOP_TAG_IMAGE.text(locale),
            Self::RemoveImage { .. } | Self::RemoveImages { .. } => {
                gui::GUI_DESKTOP_REMOVE_IMAGE.text(locale)
            }
            Self::PruneImages { dangling: true } => {
                gui::GUI_DESKTOP_PRUNE_DANGLING_IMAGES.text(locale)
            }
            Self::PruneImages { dangling: false } => {
                gui::GUI_DESKTOP_PRUNE_UNUSED_IMAGES.text(locale)
            }
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
            Self::PushImage { reference, .. } | Self::ExportImage { reference, .. } => {
                Some(reference)
            }
            Self::NetworkMembership { name, .. } => Some(name),
            Self::ImportImage
            | Self::CreateStack { .. }
            | Self::DeployStack
            | Self::PruneImages { .. }
            | Self::Pull
            | Self::CreateVolume
            | Self::CreateNetwork => None,
            Self::Run { reference, .. }
            | Self::RemoveImage { reference, .. }
            | Self::TagImage { reference, .. } => Some(reference),
            Self::RemoveImages { items } => {
                items.first().map(|(_, reference, _)| reference.as_str())
            }
            Self::RemoveVolume { name, .. } | Self::RemoveNetwork { name, .. } => Some(name),
            Self::ComposeUp { project }
            | Self::ComposeRestart { project }
            | Self::ComposeDown { project, .. } => Some(project),
        }
    }

    fn input_label(&self, locale: Locale) -> Option<&'static str> {
        match self {
            Self::CreateStack { .. } | Self::DeployStack => {
                Some(gui::GUI_DESKTOP_STACK_NAME.text(locale))
            }
            Self::NetworkMembership { .. } => Some(gui::GUI_DESKTOP_NETWORK_CONTAINER.text(locale)),
            Self::TagImage { .. } => Some(gui::GUI_DESKTOP_IMAGE_REFERENCE.text(locale)),
            Self::Pull => None,
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

    fn help(&self) -> Option<marsa_ui::TextKey> {
        match self {
            Self::CreateStack { .. } => Some(gui::GUI_COMPOSE_NEW_STACK_HELP),
            Self::DeployStack => {
                Some(gui::GUI_DESKTOP_CHOOSE_THE_COMPOSE_FILE_FOR_THIS_PROJECT_AFTER_CONFIRMING)
            }
            Self::RemoveVolume { in_use: false, .. } | Self::ComposeDown { volumes: true, .. } => {
                Some(gui::GUI_SYSTEM_ACTIVITY_DESTROY_EXPLANATION)
            }
            Self::PruneImages { dangling: true } => {
                Some(gui::GUI_DESKTOP_PRUNE_DANGLING_IMAGES_HELP)
            }
            Self::PruneImages { dangling: false } => {
                Some(gui::GUI_DESKTOP_PRUNE_UNUSED_IMAGES_HELP)
            }
            _ => None,
        }
    }

    fn users(&self) -> &[String] {
        match self {
            Self::RemoveImage { users, .. }
            | Self::RemoveVolume { users, .. }
            | Self::RemoveNetwork { users, .. } => users,
            Self::RemoveImages { items } => items
                .iter()
                .find(|(_, _, users)| !users.is_empty())
                .map_or(&[], |(_, _, users)| users.as_slice()),
            _ => &[],
        }
    }

    pub(super) fn destructive(&self) -> bool {
        matches!(
            self,
            Self::RemoveImage { .. }
                | Self::RemoveImages { .. }
                | Self::PruneImages { .. }
                | Self::RemoveVolume { .. }
                | Self::RemoveNetwork { .. }
                | Self::NetworkMembership { attach: false, .. }
                | Self::ComposeDown { .. }
        )
    }
}

/// An operation emitted only after its applicable confirmation has passed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResourceRequest {
    ImportImage,
    PushImage {
        id: String,
        reference: String,
    },
    ExportImage(String),
    NetworkMembership {
        network: String,
        container: marsa_client::docker::ContainerId,
        attach: bool,
    },
    CreateStack {
        project: String,
        yaml: String,
    },
    DeployStack {
        project: String,
        keychain: bool,
    },
    Run {
        id: String,
        name: Option<String>,
    },
    TagImage {
        id: String,
        destination: String,
    },
    RemoveImage {
        id: String,
        force: bool,
    },
    RemoveImages {
        ids: Vec<String>,
        force: bool,
    },
    PruneImages {
        dangling: bool,
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
    source: Option<super::ResourceSource>,
    input: Option<Entity<InputState>>,
    compose_editor: Option<Entity<EditorState>>,
    force: bool,
    keychain: bool,
}

impl Form {
    pub(super) fn new(
        kind: FormKind,
        source: Option<super::ResourceSource>,
        locale: Locale,
        window: &mut Window,
        cx: &mut Context<ResourcesView>,
    ) -> Self {
        let input = kind.input_label(locale).map(|_| {
            let input = cx.new(|cx| {
                let state = InputState::new(window, cx);
                if matches!(kind, FormKind::TagImage { .. }) {
                    state.placeholder(gui::GUI_DESKTOP_TARGET_IMAGE_TAG.text(locale))
                } else {
                    state
                }
            });
            cx.subscribe(
                &input,
                |_, _, _: &marsa_ui::gpui_component::input::InputEvent, cx| cx.refresh_windows(),
            )
            .detach();
            input
        });
        let compose_editor = if let FormKind::CreateStack { template } = &kind {
            let editor = cx.new(|cx| EditorState::new(window, cx).default_value(template.clone()));
            cx.subscribe(&editor, |_, _, _: &InputEvent, cx| cx.refresh_windows())
                .detach();
            Some(editor)
        } else {
            None
        };
        Self {
            kind,
            source,
            input,
            compose_editor,
            force: false,
            keychain: false,
        }
    }

    fn request(&self, state: &crate::store::AppState, cx: &App) -> Option<ResourceRequest> {
        let value = self
            .input
            .as_ref()
            .map(|input| input.read(cx).value())
            .unwrap_or_default();
        let trimmed = value.trim();
        let nonempty = || (!trimmed.is_empty()).then_some(trimmed);
        Some(match &self.kind {
            FormKind::CreateStack { .. } => {
                let project = nonempty()?.to_owned();
                let yaml = self.compose_editor.as_ref()?.read(cx).value();
                if !valid_project_name(&project) || !valid_compose_template(&yaml) {
                    return None;
                }
                ResourceRequest::CreateStack {
                    project,
                    yaml: yaml.to_string(),
                }
            }
            FormKind::DeployStack => {
                let project = nonempty()?;
                if !valid_project_name(project) {
                    return None;
                }
                ResourceRequest::DeployStack {
                    project: project.to_owned(),
                    keychain: self.keychain,
                }
            }
            FormKind::ImportImage => ResourceRequest::ImportImage,
            FormKind::PushImage { id, reference } => ResourceRequest::PushImage {
                id: id.clone(),
                reference: reference.clone(),
            },
            FormKind::ExportImage { id, .. } => ResourceRequest::ExportImage(id.clone()),
            FormKind::NetworkMembership { id, attach, .. } => {
                let selection = nonempty()?;
                let container = state
                    .containers()
                    .values()
                    .find(|container| {
                        container.id.as_str() == selection
                            || container
                                .names
                                .iter()
                                .any(|name| name.trim_start_matches('/') == selection)
                    })?
                    .id
                    .clone();
                ResourceRequest::NetworkMembership {
                    network: id.clone(),
                    container,
                    attach: *attach,
                }
            }
            FormKind::Pull => return None,
            FormKind::Run { id, .. } => ResourceRequest::Run {
                id: id.clone(),
                name: nonempty().map(str::to_owned),
            },
            FormKind::TagImage { id, .. } => ResourceRequest::TagImage {
                id: id.clone(),
                destination: nonempty()?.to_owned(),
            },
            FormKind::RemoveImage { id, users, .. } if users.is_empty() || self.force => {
                ResourceRequest::RemoveImage {
                    id: id.clone(),
                    force: self.force,
                }
            }
            FormKind::RemoveImages { items }
                if items.iter().all(|(_, _, users)| users.is_empty()) || self.force =>
            {
                ResourceRequest::RemoveImages {
                    ids: items.iter().map(|(id, _, _)| id.clone()).collect(),
                    force: self.force,
                }
            }
            FormKind::PruneImages { dangling } => ResourceRequest::PruneImages {
                dangling: *dangling,
            },
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
    ) -> (Div, Div) {
        let creating = matches!(self.kind, FormKind::CreateStack { .. });
        let title = if creating {
            gui::GUI_COMPOSE_CREATE_AND_DEPLOY.text(screen.locale)
        } else {
            self.kind.title(screen.locale)
        };
        let mut form = div()
            .flex()
            .flex_col()
            .gap(px(Space::S16.pixels()))
            .child(dialog_scope(
                screen.locale,
                screen.palette,
                self.source
                    .as_ref()
                    .map_or_else(|| "Marsa".into(), |source| source.badge.clone()),
            ));
        if let Some(target) = self.kind.target() {
            let affected = self.kind.users();
            form = form.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::S8.pixels()))
                    .p(px(Space::S12.pixels()))
                    .rounded(px(Radius::R8.pixels()))
                    .border_1()
                    .border_color(tone(screen, ColorToken::BorderSubtle))
                    .bg(tone(screen, ColorToken::BgCode))
                    .child(text(screen, Role::Mono, target.to_owned()))
                    .children((!affected.is_empty()).then(|| {
                        text(
                            screen,
                            Role::Small,
                            message(screen.locale, gui::GUI_DESKTOP_USED_BY_THESE_CONTAINERS),
                        )
                        .text_color(tone(screen, ColorToken::TextMuted))
                    }))
                    .children(
                        affected
                            .iter()
                            .map(|name| text(screen, Role::Mono, name.clone())),
                    ),
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
                .text_color(tone(screen, ColorToken::StatusWarning))
                .bg(tone(screen, ColorToken::StatusWarningBg))
                .rounded(px(Radius::R8.pixels()))
                .p(px(Space::S12.pixels())),
            );
        }
        if matches!(
            self.kind,
            FormKind::RemoveImage { .. } | FormKind::RemoveImages { .. }
        ) && !self.kind.users().is_empty()
        {
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
        form = form.children(self.kind.help().map(|key| {
            text(screen, Role::Body, message(screen.locale, key))
                .text_color(tone(screen, ColorToken::TextMuted))
        }));
        if let Some(input) = &self.input
            && let Some(label) = self.kind.input_label(screen.locale)
        {
            form = form.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Space::S4.pixels()))
                    .child(text(screen, Role::Small, label).font_weight(FontWeight::MEDIUM))
                    .child(crate::screens::common::resource_input(input).aria_label(label)),
            );
        }
        if matches!(self.kind, FormKind::DeployStack) {
            let view = view.clone();
            form = form
                .child(
                    Checkbox::new("compose-keychain")
                        .label(gui::GUI_DEPLOY_SECRETS_USE.text(screen.locale))
                        .checked(self.keychain)
                        .on_click(move |checked, _, cx| {
                            view.update(cx, |view, cx| {
                                if let Some(form) = &mut view.form {
                                    form.keychain = *checked;
                                    cx.refresh_windows();
                                }
                            });
                        }),
                )
                .children(self.keychain.then(|| {
                    text(
                        screen,
                        Role::Small,
                        gui::GUI_DEPLOY_SECRETS_CONFIRM.text(screen.locale),
                    )
                    .text_color(tone(screen, ColorToken::TextMuted))
                }));
        }
        if let Some(editor) = &self.compose_editor {
            let invalid_project = self.input.as_ref().is_some_and(|input| {
                let value = input.read(cx).value();
                !value.trim().is_empty() && !valid_project_name(value.trim())
            });
            let invalid_yaml = !valid_compose_template(editor.read(cx).value().as_ref());
            form = form
                .children(invalid_project.then(|| {
                    text(
                        screen,
                        Role::Small,
                        gui::GUI_COMPOSE_PROJECT_NAME_HELP.text(screen.locale),
                    )
                    .text_color(tone(screen, ColorToken::StatusWarning))
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::S4.pixels()))
                        .child(
                            text(
                                screen,
                                Role::Small,
                                gui::GUI_COMPOSE_YAML.text(screen.locale),
                            )
                            .font_weight(FontWeight::MEDIUM),
                        )
                        .child(
                            Editor::new(editor)
                                .h(px(224.))
                                .aria_label(gui::GUI_COMPOSE_YAML.text(screen.locale)),
                        ),
                )
                .children(invalid_yaml.then(|| {
                    text(
                        screen,
                        Role::Small,
                        gui::GUI_COMPOSE_TEMPLATE_INVALID.text(screen.locale),
                    )
                    .text_color(tone(screen, ColorToken::StatusWarning))
                }));
        }
        let cancel = view.clone();
        let submit = view.clone();
        let service = screen.services.cloned();
        let locale = screen.locale;
        let request = self.request(screen.state, cx);
        let snapshot = screen.state.clone();
        let submit_button = Button::new("submit-resource-action")
            .h(px(Space::S36.pixels()))
            .rounded(px(Radius::R8.pixels()))
            .label(title)
            .disabled(request.is_none() || service.is_none() || view.read(cx).busy.is_some());
        let submit_button = if self.kind.destructive() {
            submit_button.danger()
        } else {
            submit_button.primary()
        };
        let actions = dialog_actions(screen.locale, screen.palette)
            .child(
                Button::new("cancel-resource-action")
                    .outline()
                    .h(px(Space::S36.pixels()))
                    .rounded(px(Radius::R8.pixels()))
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
                    let request = (
                        form.kind.page(),
                        form.request(&snapshot, cx)?,
                        form.source.as_ref().map(|source| source.endpoint.clone()),
                    );
                    view.form = None;
                    Some(request)
                });
                if let Some((kind, request, endpoint)) = request {
                    window.close_dialog(cx);
                    submit.update(cx, |view, cx| {
                        view.dispatch(kind, (request, endpoint), service.clone(), locale, cx)
                    });
                }
            }));
        (form, actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_help_distinguishes_deployment_from_lifecycle_and_volume_removal() {
        assert_eq!(
            FormKind::DeployStack.help(),
            Some(gui::GUI_DESKTOP_CHOOSE_THE_COMPOSE_FILE_FOR_THIS_PROJECT_AFTER_CONFIRMING)
        );
        assert_eq!(
            FormKind::ComposeRestart {
                project: "demo".into()
            }
            .help(),
            None
        );
        assert_eq!(
            FormKind::ComposeDown {
                project: "demo".into(),
                volumes: true
            }
            .help(),
            Some(gui::GUI_SYSTEM_ACTIVITY_DESTROY_EXPLANATION)
        );
    }
}
