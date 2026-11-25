//! Render and collect the saved connection form.

use std::{net::IpAddr, path::PathBuf, sync::Arc};

use marsa_ui::gpui::Subscription;

use marsa_client::docker::{
    HAWSER_EDGE_PATH, ManagedEndpoint, ManagedEndpointConfig, ManagedEndpointTransport,
};
use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        AppContext as _, Context, Div, Entity, FontWeight, ParentElement as _, PathPromptOptions,
        Styled as _, Window, div, prelude::FluentBuilder as _, px,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, Selectable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
    preferences::EnvironmentIcon,
    tokens::typography::{Role, ui_size},
};

use super::{SystemView, feedback_text, security::Security, settings_text, vm::VmEditor};
use crate::{screens::external_inventory::ExternalWorkspace, store::AppState};

#[derive(Clone)]
pub(super) struct Appearance {
    locale: Locale,
    palette: Palette,
    state: AppState,
}

impl From<&ScreenContext<'_>> for Appearance {
    fn from(screen: &ScreenContext<'_>) -> Self {
        Self {
            locale: screen.locale,
            palette: screen.palette,
            state: screen.state.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) enum EditorTarget {
    Add,
    Saved(Box<ManagedEndpoint>, Option<Arc<ExternalWorkspace>>),
    Discovered(Arc<ExternalWorkspace>),
    Builtin,
}

impl EditorTarget {
    pub(super) fn workspace(&self) -> Option<&ExternalWorkspace> {
        match self {
            Self::Saved(_, workspace) => workspace.as_deref(),
            Self::Discovered(workspace) => Some(workspace),
            _ => None,
        }
    }

    pub(super) fn is_discovered(&self) -> bool {
        matches!(self, Self::Discovered(_))
    }

    fn editable_connection(&self) -> bool {
        matches!(self, Self::Saved(..) | Self::Add)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Resources,
    Network,
    Security,
}

impl Section {
    fn icon(self) -> &'static str {
        match self {
            Self::General => "icons/sliders-horizontal.svg",
            Self::Resources => "icons/cpu.svg",
            Self::Network => "icons/network.svg",
            Self::Security => "icons/shield-check.svg",
        }
    }

    fn label(self) -> TextKey {
        match self {
            Self::General => gui::GUI_ENVIRONMENT_TAB_GENERAL,
            Self::Resources => gui::GUI_ENVIRONMENT_TAB_RESOURCES,
            Self::Network => gui::GUI_ENVIRONMENT_TAB_NETWORK,
            Self::Security => gui::GUI_ENVIRONMENT_TAB_SECURITY,
        }
    }
}
use crate::screens::common::{
    ScreenContext, align, color, dialog_actions, dialog_frame, resource_input, row,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[repr(usize)]
enum Transport {
    #[default]
    Unix,
    Ssh,
    Tls,
    HawserStandard,
    HawserEdge,
}

impl Transport {
    fn title(self, locale: Locale) -> &'static str {
        match self {
            Self::Unix => gui::GUI_ENVIRONMENT_UNIX.text(locale),
            Self::Ssh => gui::GUI_ENVIRONMENT_SSH.text(locale),
            Self::Tls => gui::GUI_ENVIRONMENT_TLS.text(locale),
            Self::HawserStandard => gui::GUI_ENVIRONMENT_HAWSER_STANDARD.text(locale),
            Self::HawserEdge => gui::GUI_ENVIRONMENT_HAWSER_EDGE.text(locale),
        }
    }

    fn address_label(self) -> TextKey {
        match self {
            Self::Unix => gui::GUI_ENVIRONMENT_SOCKET_PATH,
            Self::Ssh => gui::GUI_ENVIRONMENT_SSH_ADDRESS,
            Self::Tls => gui::GUI_ENVIRONMENT_TLS_ADDRESS,
            Self::HawserStandard => gui::GUI_ENVIRONMENT_HAWSER_ADDRESS,
            Self::HawserEdge => gui::GUI_ENVIRONMENT_HAWSER_LISTEN,
        }
    }
}

pub(super) struct Editor {
    pub(super) id: Option<String>,
    pub(super) target: EditorTarget,
    selected_socket: Option<Arc<ExternalWorkspace>>,
    pub(super) vm: VmEditor,
    pub(super) security: Security,
    section: Section,
    transport: Transport,
    direct_transport: Transport,
    name: Entity<InputState>,
    icon: EnvironmentIcon,
    addresses: [Entity<InputState>; 5],
    agent_id: Entity<InputState>,
    standard_token: Entity<InputState>,
    edge_token: Entity<InputState>,
    edge_cert: Entity<InputState>,
    edge_key: Entity<InputState>,
    ca: Entity<InputState>,
    standard_ca: Entity<InputState>,
    cert: Entity<InputState>,
    key: Entity<InputState>,
    labels: Vec<String>,
    label: Entity<InputState>,
    public_ip: Entity<InputState>,
    _edit_observers: Vec<Subscription>,
}

impl Editor {
    pub(super) fn new(
        target: EditorTarget,
        name_and_icon: (String, EnvironmentIcon),
        window: &mut Window,
        cx: &mut Context<SystemView>,
    ) -> Self {
        let existing = match &target {
            EditorTarget::Saved(entry, _) => Some(entry),
            _ => None,
        };
        let (transport, address, ca, cert, key, agent_id) =
            match existing.map(|entry| &entry.config.transport) {
                Some(ManagedEndpointTransport::Unix { socket }) => (
                    Transport::Unix,
                    socket.to_string_lossy().into_owned(),
                    None,
                    None,
                    None,
                    None,
                ),
                Some(ManagedEndpointTransport::Ssh { address }) => {
                    (Transport::Ssh, address.clone(), None, None, None, None)
                }
                Some(ManagedEndpointTransport::Tls {
                    address,
                    ca,
                    cert,
                    key,
                }) => (
                    Transport::Tls,
                    address.clone(),
                    Some(ca),
                    Some(cert),
                    Some(key),
                    None,
                ),
                Some(ManagedEndpointTransport::HawserStandard { address, ca, .. }) => (
                    Transport::HawserStandard,
                    address.clone(),
                    Some(ca),
                    None,
                    None,
                    None,
                ),
                Some(ManagedEndpointTransport::HawserEdge {
                    bind,
                    cert,
                    key,
                    agent_id,
                    ..
                }) => (
                    Transport::HawserEdge,
                    bind.to_string(),
                    None,
                    Some(cert),
                    Some(key),
                    Some(agent_id.as_str()),
                ),
                None => (Transport::Unix, String::new(), None, None, None, None),
            };
        let field = |value: &str, window: &mut Window, cx: &mut Context<SystemView>| {
            let field = cx.new(|cx| InputState::new(window, cx));
            field.update(cx, |field, cx| {
                field.set_value(value.to_owned(), window, cx)
            });
            field
        };
        let path_field =
            |path: Option<&PathBuf>, window: &mut Window, cx: &mut Context<SystemView>| {
                field(
                    &path.map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
                    window,
                    cx,
                )
            };
        let (display_name, icon) = name_and_icon;
        let name = field(&display_name, window, cx);
        let addresses = std::array::from_fn(|index| {
            field(
                if index == transport as usize {
                    &address
                } else {
                    ""
                },
                window,
                cx,
            )
        });
        let standard_ca = path_field(
            ca.filter(|_| transport == Transport::HawserStandard),
            window,
            cx,
        );
        let ca = path_field(ca.filter(|_| transport == Transport::Tls), window, cx);
        let edge_cert = path_field(
            cert.filter(|_| transport == Transport::HawserEdge),
            window,
            cx,
        );
        let edge_key = path_field(
            key.filter(|_| transport == Transport::HawserEdge),
            window,
            cx,
        );
        let cert = path_field(cert.filter(|_| transport == Transport::Tls), window, cx);
        let key = path_field(key.filter(|_| transport == Transport::Tls), window, cx);
        let agent_id = field(agent_id.unwrap_or_default(), window, cx);
        let standard_token = field("", window, cx);
        let edge_token = field("", window, cx);
        for token in [&standard_token, &edge_token] {
            token.update(cx, |field, cx| field.set_masked(true, window, cx));
        }
        let labels = existing.map_or_else(Vec::new, |entry| entry.config.labels.clone());
        let label = field("", window, cx);
        let public_ip = field(
            &existing
                .and_then(|entry| entry.config.public_ip)
                .map_or_else(String::new, |ip| ip.to_string()),
            window,
            cx,
        );
        let _edit_observers = addresses
            .iter()
            .chain([
                &name,
                &ca,
                &standard_ca,
                &cert,
                &key,
                &label,
                &public_ip,
                &agent_id,
                &standard_token,
                &edge_token,
                &edge_cert,
                &edge_key,
            ])
            .map(|field| {
                cx.subscribe(field, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change)
                        && this.environment_settings.editor.is_some()
                    {
                        this.environment_settings.feedback = None;
                        cx.refresh_windows();
                    }
                })
            })
            .collect();
        Self {
            id: existing.map(|entry| entry.id.clone()),
            transport,
            direct_transport: if transport == Transport::Tls {
                Transport::Tls
            } else {
                Transport::Ssh
            },
            name,
            icon,
            addresses,
            agent_id,
            standard_token,
            edge_token,
            edge_cert,
            edge_key,
            ca,
            standard_ca,
            cert,
            key,
            labels,
            label,
            public_ip,
            vm: VmEditor::new(&target, window, cx),
            security: Security::default(),
            target,
            selected_socket: None,
            section: Section::General,
            _edit_observers,
        }
    }

    pub(super) fn config(
        &self,
        cx: &Context<SystemView>,
    ) -> Result<ManagedEndpointConfig, TextKey> {
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let transport = self.draft_transport(cx)?;
        let public_ip = value(&self.public_ip);
        let public_ip = if public_ip.is_empty() {
            None
        } else {
            Some(
                public_ip
                    .parse::<IpAddr>()
                    .ok()
                    .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
                    .ok_or(gui::GUI_ENVIRONMENT_INVALID_PUBLIC_IP)?,
            )
        };
        let mut labels = self.labels.clone();
        let label = value(&self.label);
        if !label.is_empty() && !labels.contains(&label) {
            labels.push(label);
        }
        Ok(ManagedEndpointConfig {
            icon: Some(super::icon_id(self.icon).to_owned()),
            name: value(&self.name),
            transport,
            labels,
            public_ip,
        })
    }

    fn address(&self) -> &Entity<InputState> {
        &self.addresses[self.transport as usize]
    }

    pub(super) fn hawser_token(&self, cx: &Context<SystemView>) -> Option<String> {
        let input = match self.transport {
            Transport::HawserStandard => &self.standard_token,
            Transport::HawserEdge => &self.edge_token,
            _ => return None,
        };
        let token = input.read(cx).value();
        let token = token.trim();
        (!token.is_empty()).then(|| token.to_owned())
    }

    fn draft_transport(
        &self,
        cx: &Context<SystemView>,
    ) -> Result<ManagedEndpointTransport, TextKey> {
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let address = value(self.address());
        let existing = match &self.target {
            EditorTarget::Saved(entry, _) => Some(&entry.config.transport),
            _ => None,
        };
        Ok(match self.transport {
            Transport::Unix => ManagedEndpointTransport::Unix {
                socket: address.into(),
            },
            Transport::Ssh => ManagedEndpointTransport::Ssh { address },
            Transport::Tls => ManagedEndpointTransport::Tls {
                address,
                ca: value(&self.ca).into(),
                cert: value(&self.cert).into(),
                key: value(&self.key).into(),
            },
            Transport::HawserStandard => {
                let ca = PathBuf::from(value(&self.standard_ca));
                let credential = match existing {
                    Some(ManagedEndpointTransport::HawserStandard {
                        address: saved_address,
                        ca: saved_ca,
                        credential,
                    }) if *saved_address == address && *saved_ca == ca => credential.clone(),
                    _ => None,
                };
                if credential.is_none() && self.hawser_token(cx).is_none() {
                    return Err(gui::GUI_ENVIRONMENT_HAWSER_TOKEN_REQUIRED);
                }
                ManagedEndpointTransport::HawserStandard {
                    address,
                    ca,
                    credential,
                }
            }
            Transport::HawserEdge => {
                let bind = address
                    .parse()
                    .map_err(|_| gui::GUI_ENVIRONMENT_INVALID_ADDRESS)?;
                let cert = PathBuf::from(value(&self.edge_cert));
                let key = PathBuf::from(value(&self.edge_key));
                let agent_id = value(&self.agent_id);
                let credential = match existing {
                    Some(ManagedEndpointTransport::HawserEdge {
                        bind: saved_bind,
                        cert: saved_cert,
                        key: saved_key,
                        agent_id: saved_id,
                        credential,
                    }) if *saved_bind == bind
                        && *saved_cert == cert
                        && *saved_key == key
                        && *saved_id == agent_id =>
                    {
                        credential.clone()
                    }
                    _ => None,
                };
                if credential.is_none() && self.hawser_token(cx).is_none() {
                    return Err(gui::GUI_ENVIRONMENT_HAWSER_TOKEN_REQUIRED);
                }
                ManagedEndpointTransport::HawserEdge {
                    bind,
                    cert,
                    key,
                    agent_id,
                    credential,
                }
            }
        })
    }

    pub(super) fn provider_matches_draft(&self, cx: &Context<SystemView>) -> bool {
        match &self.target {
            EditorTarget::Saved(entry, _) => self
                .draft_transport(cx)
                .is_ok_and(|transport| entry.config.transport == transport),
            EditorTarget::Add => match self.draft_transport(cx) {
                Ok(ManagedEndpointTransport::Unix { socket }) => {
                    self.selected_socket
                        .as_ref()
                        .and_then(|workspace| workspace.endpoint.socket())
                        == Some(socket.as_path())
                }
                _ => false,
            },
            _ => true,
        }
    }

    pub(super) fn workspace(&self, cx: &Context<SystemView>) -> Option<&ExternalWorkspace> {
        if self.provider_matches_draft(cx) {
            self.target.workspace().or(self.selected_socket.as_deref())
        } else {
            None
        }
    }
}

impl SystemView {
    pub(super) fn open_environment_editor(
        &mut self,
        target: EditorTarget,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.environment_settings.editor.is_some() || self.environment_settings.task.is_some() {
            return;
        }
        if matches!(target, EditorTarget::Builtin) {
            self.show_resource_inputs(window, cx);
        }
        let presentation = self.editor_presentation(&target);
        self.environment_settings.editor = Some(Editor::new(target, presentation, window, cx));
        self.environment_settings.feedback = None;
        self.load_environment_vm(window, cx);
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (title, identity, busy, tabs, body, footer) = owner
                .update(cx, |this, cx| {
                    let Some(editor) = &this.environment_settings.editor else {
                        return (
                            gui::GUI_ENVIRONMENT_EDIT_TITLE,
                            None,
                            false,
                            None,
                            div(),
                            div(),
                        );
                    };
                    let screen = ScreenContext {
                        locale: appearance.locale,
                        palette: appearance.palette,
                        display: this.display_settings(),
                        state: &appearance.state,
                        services: this.services.as_ref(),
                    };
                    let title = if matches!(editor.target, EditorTarget::Add) {
                        gui::GUI_ENVIRONMENT_ADD
                    } else {
                        gui::GUI_ENVIRONMENT_EDIT_TITLE
                    };
                    let busy = this.environment_settings.task.is_some()
                        || editor.vm.applying()
                        || this.busy();
                    let identity = (!matches!(editor.target, EditorTarget::Add))
                        .then(|| this.editor_presentation(&editor.target).0);
                    let (tabs, body) = this.environment_dialog_content(editor, &screen, cx);
                    let footer = this.environment_dialog_footer(editor, &screen, busy, cx);
                    (title, identity, busy, tabs, body, footer)
                })
                .unwrap_or_else(|_| {
                    (
                        gui::GUI_ENVIRONMENT_EDIT_TITLE,
                        None,
                        false,
                        None,
                        div(),
                        div(),
                    )
                });
            let locale = appearance.locale;
            let close = owner.clone();
            let close_button = owner.clone();
            let heading = row(locale)
                .w_full()
                .items_center()
                .justify_between()
                .gap_3()
                .pb_3()
                .border_b_1()
                .border_color(color(appearance.palette, ColorToken::BorderSubtle))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .flex_1()
                        .child(
                            align(
                                div()
                                    .text_size(ui_size(20.))
                                    .font_weight(FontWeight::SEMIBOLD),
                                locale,
                            )
                            .text_color(color(appearance.palette, ColorToken::TextPrimary))
                            .child(title.text(locale)),
                        )
                        .children(identity.map(|name| {
                            align(div().text_size(ui_size(12.)), locale)
                                .truncate()
                                .text_color(color(appearance.palette, ColorToken::TextMuted))
                                .child(crate::screens::common::technical(&name))
                        })),
                )
                .child(
                    Button::new("close-environment-editor")
                        .ghost()
                        .icon(IconName::Close)
                        .accessibility_label(gui::GUI_DESKTOP_CLOSE.text(locale))
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            let _ = close_button
                                .update(cx, |this, cx| this.dismiss_environment_editor(cx));
                            window.close_dialog(cx);
                        }),
                );
            let height = (window.viewport_size().height - px(56.)).clamp(px(0.), px(700.));
            dialog_frame(dialog, 820.)
                .h(height)
                .title(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(heading)
                        .children(tabs),
                )
                .overlay_closable(false)
                .close_button(false)
                .keyboard(!busy)
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| this.dismiss_environment_editor(cx));
                })
                .child(body)
                .footer(footer)
        });
        cx.notify();
    }

    fn dismiss_environment_editor(&mut self, cx: &mut Context<Self>) {
        self.environment_settings.editor = None;
        self.environment_settings.feedback = None;
        cx.notify();
    }

    fn environment_dialog_content(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> (Option<Div>, Div) {
        let locale = screen.locale;
        let builtin = matches!(editor.target, EditorTarget::Builtin);
        let mut sections = vec![Section::General];
        if builtin || (editor.vm.available() && editor.provider_matches_draft(cx)) {
            sections.extend([Section::Resources, Section::Network]);
        }
        if !matches!(editor.target, EditorTarget::Add) {
            sections.push(Section::Security);
        }
        let section = if sections.contains(&editor.section) {
            editor.section
        } else {
            Section::General
        };
        let tabs = row(locale)
            .gap_1()
            .flex_wrap()
            .children(sections.iter().copied().map(|tab_section| {
                Button::new(("environment-section", tab_section as usize))
                    .ghost()
                    .icon(Icon::default().path(tab_section.icon()))
                    .selected(tab_section == section)
                    .toggled(tab_section == section)
                    .when(tab_section == section, |tab| {
                        tab.bg(color(screen.palette, ColorToken::AccentSubtle))
                    })
                    .label(tab_section.label().text(locale))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(editor) = &mut this.environment_settings.editor {
                            editor.section = tab_section;
                        }
                        if tab_section == Section::Security {
                            this.load_environment_security(window, cx);
                        }
                        cx.notify();
                    }))
            }));
        let page = match section {
            Section::General => self.environment_general(editor, screen, cx),
            Section::Resources if builtin => self.resource_editor(screen, cx),
            Section::Network if builtin => self.network(screen, cx),
            Section::Resources => self.environment_vm(editor, false, screen, cx),
            Section::Network => self.environment_vm(editor, true, screen, cx),
            Section::Security => self.environment_security(editor, screen, cx),
        };
        ((sections.len() > 1).then_some(tabs), page)
    }

    fn environment_dialog_footer(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        busy: bool,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let editable = editor.target.editable_connection();
        let connection = row(locale)
            .gap_2()
            .flex_wrap()
            .children(
                (editable
                    || editor.target.is_discovered()
                    || matches!(editor.target, EditorTarget::Builtin))
                .then(|| {
                    Button::new("test-environment")
                        .outline()
                        .icon(Icon::default().path("icons/network.svg"))
                        .label(gui::GUI_ENVIRONMENT_TEST.text(locale))
                        .disabled(busy || self.services.is_none())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.environment_submit(true, window, cx)
                        }))
                }),
            )
            .children(editor.id.as_ref().map(|id| {
                let id = id.clone();
                Button::new("remove-environment-dialog")
                    .ghost()
                    .icon(Icon::default().path("icons/trash.svg"))
                    .tooltip(gui::GUI_DESKTOP_REMOVE.text(locale))
                    .accessibility_label(gui::GUI_DESKTOP_REMOVE.text(locale))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm_environment_removal(id.clone(), locale, window, cx);
                    }))
            }));
        let commit = row(locale)
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("cancel-environment")
                    .outline()
                    .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dismiss_environment_editor(cx);
                        window.close_dialog(cx);
                    })),
            )
            .child({
                Button::new("save-environment")
                    .primary()
                    .label(
                        if matches!(editor.target, EditorTarget::Add) {
                            gui::GUI_ENVIRONMENT_ADD
                        } else {
                            gui::GUI_ENVIRONMENT_SAVE
                        }
                        .text(locale),
                    )
                    .disabled(
                        busy || self.services.is_none() || (!editable && self.settings.is_none()),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.environment_submit(false, window, cx)
                        }),
                    )
            });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(self.environment_settings.task.is_some().then(|| {
                settings_text(
                    screen,
                    Role::Small,
                    gui::GUI_DESKTOP_WORKING.text(locale),
                    ColorToken::TextMuted,
                )
            }))
            .children(
                self.environment_settings
                    .feedback
                    .as_ref()
                    .map(|feedback| feedback_text(screen, feedback)),
            )
            .child(
                dialog_actions(locale, screen.palette)
                    .justify_between()
                    .child(connection)
                    .child(commit),
            )
    }

    fn environment_general(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
        let busy = self.environment_settings.task.is_some() || editor.vm.applying() || self.busy();
        let identity_fields = self.environment_identity_fields(editor, screen, busy, cx);
        if !editor.target.editable_connection() {
            let value = editor
                .target
                .workspace()
                .map(|workspace| workspace.endpoint.address())
                .or_else(|| {
                    marsa_client::docker_socket_path()
                        .ok()
                        .map(|path| path.display().to_string())
                });
            let save_discovered_socket = editor
                .target
                .workspace()
                .and_then(|workspace| Some(workspace.endpoint.socket()?.to_str()?.to_owned()));
            return div()
                .flex()
                .flex_col()
                .gap_3()
                .child(identity_fields)
                .child(environment_group(
                    screen,
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(settings_text(
                            screen,
                            Role::Label,
                            gui::GUI_ENVIRONMENT_CONNECTION.text(screen.locale),
                            ColorToken::TextPrimary,
                        ))
                        .children(value.filter(|value| !value.is_empty()).map(|value| {
                            settings_text(
                                screen,
                                Role::Mono,
                                crate::screens::common::technical(&value),
                                ColorToken::TextMuted,
                            )
                        }))
                        .child(settings_text(
                            screen,
                            Role::Small,
                            if editor.target.is_discovered() {
                                gui::GUI_ENVIRONMENT_DISCOVERED_HELP
                            } else {
                                gui::GUI_ENVIRONMENT_BUILTIN_HELP
                            }
                            .text(screen.locale),
                            ColorToken::TextMuted,
                        )),
                ))
                .children(
                    matches!(editor.target, EditorTarget::Builtin)
                        .then(|| self.docker_cli_integration(screen, cx)),
                )
                .children(
                    matches!(editor.target, EditorTarget::Builtin)
                        .then(|| self.docker_socket_integration(screen, cx)),
                )
                .children(save_discovered_socket.map(|socket| {
                    row(screen.locale).child(
                        Button::new("save-discovered-socket")
                            .outline()
                            .label(gui::GUI_ENVIRONMENT_SAVE_DISCOVERED.text(screen.locale))
                            .disabled(
                                self.services.is_none()
                                    || self.environment_settings.task.is_some()
                                    || editor.vm.applying(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let Some(editor) = &mut this.environment_settings.editor else {
                                    return;
                                };
                                let EditorTarget::Discovered(workspace) = &editor.target else {
                                    return;
                                };
                                let workspace = Arc::clone(workspace);
                                editor.address().update(cx, |address, cx| {
                                    address.set_value(socket.clone(), window, cx)
                                });
                                editor.target = EditorTarget::Add;
                                editor.selected_socket = Some(workspace);
                                this.environment_settings.feedback = None;
                                cx.notify();
                            })),
                    )
                }));
        }
        let locale = screen.locale;
        let current = editor.transport;
        let direct = matches!(current, Transport::Ssh | Transport::Tls);
        let picker = row(locale).gap_2().flex_wrap().children(
            [
                (
                    Transport::Unix,
                    Transport::Unix.title(locale),
                    "icons/plug-zap.svg",
                    current == Transport::Unix,
                ),
                (
                    editor.direct_transport,
                    gui::GUI_ENVIRONMENT_DIRECT.text(locale),
                    "icons/network.svg",
                    direct,
                ),
                (
                    Transport::HawserStandard,
                    Transport::HawserStandard.title(locale),
                    "icons/server.svg",
                    current == Transport::HawserStandard,
                ),
                (
                    Transport::HawserEdge,
                    Transport::HawserEdge.title(locale),
                    "icons/radio.svg",
                    current == Transport::HawserEdge,
                ),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (transport, label, icon, selected))| {
                transport_choice(
                    ("environment-transport", index),
                    transport,
                    label,
                    selected,
                    busy,
                    cx,
                )
                .icon(Icon::default().path(icon))
                .flex_grow(1.)
            }),
        );
        let protocol = direct.then(|| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(settings_text(
                    screen,
                    Role::Label,
                    gui::GUI_ENVIRONMENT_DIRECT_PROTOCOL.text(locale),
                    ColorToken::TextPrimary,
                ))
                .child(
                    row(locale).gap_2().children(
                        [Transport::Ssh, Transport::Tls]
                            .into_iter()
                            .enumerate()
                            .map(|(index, transport)| {
                                transport_choice(
                                    ("environment-direct-protocol", index),
                                    transport,
                                    transport.title(locale),
                                    current == transport,
                                    busy,
                                    cx,
                                )
                            }),
                    ),
                )
        });
        let hint = match current {
            Transport::Unix => gui::GUI_ENVIRONMENT_UNIX_HELP,
            Transport::Ssh => gui::GUI_ENVIRONMENT_SSH_HELP,
            Transport::Tls => gui::GUI_ENVIRONMENT_TLS_HELP,
            Transport::HawserStandard => gui::GUI_ENVIRONMENT_HAWSER_STANDARD_HELP,
            Transport::HawserEdge => gui::GUI_ENVIRONMENT_HAWSER_EDGE_HELP,
        };
        let mut connection = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_text(
                screen,
                Role::Label,
                gui::GUI_ENVIRONMENT_CONNECTION_TYPE.text(locale),
                ColorToken::TextPrimary,
            ))
            .child(picker)
            .child(settings_text(
                screen,
                Role::Small,
                hint.text(locale),
                ColorToken::TextMuted,
            ))
            .children(protocol)
            .child(input_field(
                screen,
                current.address_label(),
                editor.address(),
                busy,
                false,
                cx,
            ));
        if current == Transport::Unix {
            let choices = self
                .external_inventory
                .iter()
                .filter(|workspace| {
                    workspace.reachable || workspace.endpoint.lima_instance().is_some()
                })
                .filter_map(|workspace| {
                    Some((
                        Arc::clone(workspace),
                        workspace.endpoint.socket()?.to_str()?.to_owned(),
                    ))
                })
                .collect::<Vec<_>>();
            if !choices.is_empty() {
                let owner = cx.weak_entity();
                connection = connection.child(
                    row(locale).child(
                        Button::new("detected-docker-sockets")
                            .outline()
                            .label(gui::GUI_ENVIRONMENT_SELECT_SOCKET.text(locale))
                            .disabled(busy)
                            .dropdown_menu(move |menu, _, _| {
                                choices.iter().fold(menu, |menu, (workspace, path)| {
                                    let label = format!("{} · {path}", workspace.source);
                                    let path = path.clone();
                                    let workspace = Arc::clone(workspace);
                                    let owner = owner.clone();
                                    menu.item(PopupMenuItem::new(label).on_click(
                                        move |_, window, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                let Some(editor) =
                                                    &mut this.environment_settings.editor
                                                else {
                                                    return;
                                                };
                                                editor.address().update(cx, |address, cx| {
                                                    address.set_value(path.clone(), window, cx)
                                                });
                                                if matches!(editor.target, EditorTarget::Add) {
                                                    editor.selected_socket =
                                                        Some(Arc::clone(&workspace));
                                                    editor.vm = VmEditor::new(
                                                        &EditorTarget::Discovered(Arc::clone(
                                                            &workspace,
                                                        )),
                                                        window,
                                                        cx,
                                                    );
                                                    this.load_environment_vm(window, cx);
                                                }
                                                cx.notify();
                                            });
                                        },
                                    ))
                                })
                            }),
                    ),
                );
            }
        }
        let tls_files = match current {
            Transport::Tls => &[
                (gui::GUI_ENVIRONMENT_CA, &editor.ca),
                (gui::GUI_ENVIRONMENT_CERT, &editor.cert),
                (gui::GUI_ENVIRONMENT_KEY, &editor.key),
            ][..],
            Transport::HawserStandard => &[(gui::GUI_ENVIRONMENT_CA, &editor.standard_ca)][..],
            Transport::HawserEdge => &[
                (gui::GUI_ENVIRONMENT_HAWSER_SERVER_CERT, &editor.edge_cert),
                (gui::GUI_ENVIRONMENT_HAWSER_SERVER_KEY, &editor.edge_key),
            ][..],
            _ => &[],
        };
        if current == Transport::HawserEdge {
            let address = format!("DOCKHAND_SERVER_URL=wss://host:port{HAWSER_EDGE_PATH}");
            connection = connection
                .child(settings_text(
                    screen,
                    Role::Mono,
                    crate::screens::common::technical(&address),
                    ColorToken::TextMuted,
                ))
                .child(input_field(
                    screen,
                    gui::GUI_ENVIRONMENT_HAWSER_AGENT_ID,
                    &editor.agent_id,
                    busy,
                    false,
                    cx,
                ));
        }
        connection = connection.children(
            tls_files
                .iter()
                .map(|(label, field)| input_field(screen, *label, field, busy, true, cx)),
        );
        if matches!(current, Transport::HawserStandard | Transport::HawserEdge) {
            let token = if current == Transport::HawserStandard {
                &editor.standard_token
            } else {
                &editor.edge_token
            };
            connection = connection
                .child(input_field(
                    screen,
                    gui::GUI_ENVIRONMENT_HAWSER_TOKEN,
                    token,
                    busy,
                    false,
                    cx,
                ))
                .children(editor.id.as_ref().map(|_| {
                    settings_text(
                        screen,
                        Role::Small,
                        gui::GUI_ENVIRONMENT_HAWSER_SAVED_TOKEN.text(locale),
                        ColorToken::TextMuted,
                    )
                }));
        }
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(identity_fields)
            .child(environment_group(
                screen,
                self.environment_labels(editor, screen, busy, cx),
            ))
            .child(environment_group(screen, connection))
            .child(environment_group(
                screen,
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(input_field(
                        screen,
                        gui::GUI_ENVIRONMENT_PUBLIC_IP,
                        &editor.public_ip,
                        busy,
                        false,
                        cx,
                    ))
                    .child(settings_text(
                        screen,
                        Role::Small,
                        gui::GUI_ENVIRONMENT_PUBLIC_IP_HELP.text(locale),
                        ColorToken::TextMuted,
                    )),
            ))
    }

    fn environment_identity_fields(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let icons = [
            (EnvironmentIcon::Server, gui::GUI_ENVIRONMENT_ICON_SERVER),
            (EnvironmentIcon::Rocket, gui::GUI_ENVIRONMENT_ICON_ROCKET),
            (EnvironmentIcon::Globe, gui::GUI_ENVIRONMENT_ICON_GLOBE),
            (EnvironmentIcon::Box, gui::GUI_ENVIRONMENT_ICON_BOX),
        ];
        row(locale)
            .gap_3()
            .items_end()
            .flex_wrap()
            .child(div().flex_1().min_w(px(190.)).child(input_field(
                screen,
                gui::GUI_DESKTOP_NAME,
                &editor.name,
                disabled,
                false,
                cx,
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(settings_text(
                        screen,
                        Role::Label,
                        gui::GUI_ENVIRONMENT_ICON.text(locale),
                        ColorToken::TextPrimary,
                    ))
                    .child(
                        row(locale)
                            .gap_1()
                            .children(icons.into_iter().enumerate().map(
                                |(index, (icon, label))| {
                                    Button::new(("environment-icon", index))
                                        .outline()
                                        .icon(Icon::default().path(super::icon_path(icon)))
                                        .selected(editor.icon == icon)
                                        .toggled(editor.icon == icon)
                                        .accessibility_label(label.text(locale))
                                        .tooltip(label.text(locale))
                                        .disabled(disabled)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(editor) =
                                                &mut this.environment_settings.editor
                                            {
                                                editor.icon = icon;
                                                cx.notify();
                                            }
                                        }))
                                },
                            )),
                    ),
            )
    }

    pub(super) fn presentation_draft(
        &self,
        cx: &Context<Self>,
    ) -> Option<(EditorTarget, String, EnvironmentIcon)> {
        let editor = self.environment_settings.editor.as_ref()?;
        Some((
            editor.target.clone(),
            editor.name.read(cx).value().trim().to_owned(),
            editor.icon,
        ))
    }

    fn environment_labels(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        busy: bool,
        cx: &Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let mut group = div().flex().flex_col().gap_2().child(
            row(locale)
                .items_center()
                .gap_2()
                .child(settings_text(
                    screen,
                    Role::Label,
                    gui::GUI_ENVIRONMENT_LABELS.text(locale),
                    ColorToken::TextPrimary,
                ))
                .child(settings_text(
                    screen,
                    Role::Small,
                    format!(
                        "{}/{}",
                        editor.labels.len(),
                        ManagedEndpointConfig::MAX_LABELS
                    ),
                    ColorToken::TextMuted,
                )),
        );
        if !editor.labels.is_empty() {
            group = group.child(row(locale).gap_2().flex_wrap().children(
                editor.labels.iter().enumerate().map(|(index, label)| {
                    let a11y = format!("{} {label}", gui::GUI_DESKTOP_REMOVE.text(locale));
                    Button::new(("environment-label", index))
                        .outline()
                        .label(label.clone())
                        .icon(IconName::Close)
                        .accessibility_label(a11y)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = &mut this.environment_settings.editor
                                && index < editor.labels.len()
                            {
                                editor.labels.remove(index);
                                this.environment_settings.feedback = None;
                                cx.notify();
                            }
                        }))
                }),
            ));
        }
        let label = editor.label.read(cx).value();
        let label = label.trim();
        let limit = editor.labels.len() >= ManagedEndpointConfig::MAX_LABELS;
        group.child(
            row(locale)
                .gap_2()
                .items_center()
                .child(
                    div().flex_1().min_w_0().child(
                        resource_input(&editor.label)
                            .aria_label(gui::GUI_ENVIRONMENT_ADD_LABEL.text(locale))
                            .disabled(busy || limit),
                    ),
                )
                .child(
                    Button::new("add-environment-label")
                        .outline()
                        .icon(IconName::Plus)
                        .accessibility_label(gui::GUI_ENVIRONMENT_ADD_LABEL.text(locale))
                        .disabled(
                            busy || limit
                                || label.is_empty()
                                || editor.labels.iter().any(|existing| existing == label),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(editor) = &mut this.environment_settings.editor {
                                let label = editor.label.read(cx).value().trim().to_owned();
                                if editor.labels.len() < ManagedEndpointConfig::MAX_LABELS
                                    && !label.is_empty()
                                    && !editor.labels.contains(&label)
                                {
                                    editor.labels.push(label);
                                    editor
                                        .label
                                        .update(cx, |label, cx| label.set_value("", window, cx));
                                    this.environment_settings.feedback = None;
                                    cx.notify();
                                }
                            }
                        })),
                ),
        )
    }
}

fn transport_choice(
    id: (&'static str, usize),
    transport: Transport,
    label: &'static str,
    selected: bool,
    busy: bool,
    cx: &Context<SystemView>,
) -> Button {
    Button::new(id)
        .outline()
        .selected(selected)
        .toggled(selected)
        .label(label)
        .disabled(busy)
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(editor) = &mut this.environment_settings.editor {
                editor.transport = transport;
                if matches!(transport, Transport::Ssh | Transport::Tls) {
                    editor.direct_transport = transport;
                }
                this.environment_settings.feedback = None;
                cx.notify();
            }
        }))
}

fn environment_group(screen: &ScreenContext<'_>, content: Div) -> Div {
    div()
        .border_t_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .pt_4()
        .child(content)
}

fn input_field(
    screen: &ScreenContext<'_>,
    label: TextKey,
    input: &Entity<InputState>,
    busy: bool,
    browse: bool,
    cx: &Context<SystemView>,
) -> Div {
    let title = label.text(screen.locale);
    let locale = screen.locale;
    let field = input.clone();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(settings_text(
            screen,
            Role::Label,
            title,
            ColorToken::TextPrimary,
        ))
        .child(
            row(locale)
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(resource_input(input).aria_label(title).disabled(busy)),
                )
                .children(browse.then(|| {
                    Button::new(title)
                        .outline()
                        .label(gui::GUI_ENVIRONMENT_BROWSE.text(locale))
                        .disabled(busy)
                        .on_click(cx.listener(move |_, _, window, cx| {
                            let field = field.clone();
                            let picker = cx.prompt_for_paths(PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple: false,
                                prompt: Some(title.into()),
                            });
                            cx.spawn_in(window, async move |_, cx| {
                                if let Ok(Ok(Some(paths))) = picker.await
                                    && let Some(path) = paths.first().and_then(|path| path.to_str())
                                {
                                    let _ = cx.update(|window, cx| {
                                        field.update(cx, |input, cx| {
                                            input.set_value(path.to_owned(), window, cx);
                                        })
                                    });
                                }
                            })
                            .detach();
                        }))
                })),
        )
}
