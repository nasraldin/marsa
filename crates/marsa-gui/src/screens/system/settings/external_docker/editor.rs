//! Render and collect the saved connection form.

use std::{net::IpAddr, path::PathBuf, sync::Arc};

use marsa_ui::gpui::Subscription;

use marsa_client::docker::{ManagedEndpoint, ManagedEndpointConfig, ManagedEndpointTransport};
use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    gpui::{
        AppContext as _, Context, Div, Entity, InteractiveElement as _, ParentElement as _,
        PathPromptOptions, StatefulInteractiveElement as _, Styled as _, Window, div,
        prelude::FluentBuilder as _, px,
    },
    gpui_component::{
        Disableable as _, IconName, Selectable as _, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, feedback_text, observations::Observations, settings_text, vm::VmEditor};
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
    Saved(ManagedEndpoint, Option<Arc<ExternalWorkspace>>),
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
    Activity,
    Reports,
}

impl Section {
    fn label(self) -> TextKey {
        match self {
            Self::General => gui::GUI_ENVIRONMENT_TAB_GENERAL,
            Self::Resources => gui::GUI_ENVIRONMENT_TAB_RESOURCES,
            Self::Network => gui::GUI_ENVIRONMENT_TAB_NETWORK,
            Self::Activity => gui::GUI_ENVIRONMENT_TAB_ACTIVITY,
            Self::Reports => gui::GUI_ENVIRONMENT_TAB_REPORTS,
        }
    }
}
use crate::screens::common::{ScreenContext, color, resource_input, row};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Transport {
    #[default]
    Unix,
    Ssh,
    Tls,
}

impl Transport {
    fn title(self, locale: Locale) -> &'static str {
        match self {
            Self::Unix => gui::GUI_ENVIRONMENT_UNIX.text(locale),
            Self::Ssh => "SSH",
            Self::Tls => "TLS",
        }
    }

    fn address_label(self) -> TextKey {
        match self {
            Self::Unix => gui::GUI_ENVIRONMENT_SOCKET_PATH,
            Self::Ssh => gui::GUI_ENVIRONMENT_SSH_ADDRESS,
            Self::Tls => gui::GUI_ENVIRONMENT_TLS_ADDRESS,
        }
    }
}

pub(super) struct Editor {
    pub(super) id: Option<String>,
    pub(super) target: EditorTarget,
    selected_socket: Option<Arc<ExternalWorkspace>>,
    pub(super) vm: VmEditor,
    pub(super) observations: Observations,
    pub(super) observations_token: std::rc::Rc<()>,
    section: Section,
    transport: Transport,
    name: Entity<InputState>,
    address: Entity<InputState>,
    ca: Entity<InputState>,
    cert: Entity<InputState>,
    key: Entity<InputState>,
    labels: Vec<String>,
    label: Entity<InputState>,
    public_ip: Entity<InputState>,
    _edit_observers: [Subscription; 7],
}

impl Editor {
    pub(super) fn new(
        target: EditorTarget,
        window: &mut Window,
        cx: &mut Context<SystemView>,
    ) -> Self {
        let existing = match &target {
            EditorTarget::Saved(entry, _) => Some(entry),
            _ => None,
        };
        let (transport, address, ca, cert, key) =
            match existing.map(|entry| &entry.config.transport) {
                Some(ManagedEndpointTransport::Unix { socket }) => (
                    Transport::Unix,
                    socket.to_string_lossy().into_owned(),
                    None,
                    None,
                    None,
                ),
                Some(ManagedEndpointTransport::Ssh { address }) => {
                    (Transport::Ssh, address.clone(), None, None, None)
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
                ),
                None => (Transport::Unix, String::new(), None, None, None),
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
        let name = field(
            existing.map_or("", |entry| entry.config.name.as_str()),
            window,
            cx,
        );
        let address = field(&address, window, cx);
        let ca = path_field(ca, window, cx);
        let cert = path_field(cert, window, cx);
        let key = path_field(key, window, cx);
        let labels = existing.map_or_else(Vec::new, |entry| entry.config.labels.clone());
        let label = field("", window, cx);
        let public_ip = field(
            &existing
                .and_then(|entry| entry.config.public_ip)
                .map_or_else(String::new, |ip| ip.to_string()),
            window,
            cx,
        );
        let _edit_observers =
            [&name, &address, &ca, &cert, &key, &label, &public_ip].map(|field| {
                cx.subscribe(field, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change)
                        && this.environment_settings.editor.is_some()
                    {
                        this.environment_settings.feedback = None;
                        cx.refresh_windows();
                    }
                })
            });
        Self {
            id: existing.map(|entry| entry.id.clone()),
            transport,
            name,
            address,
            ca,
            cert,
            key,
            labels,
            label,
            public_ip,
            vm: VmEditor::new(&target, window, cx),
            observations: Observations::default(),
            observations_token: std::rc::Rc::new(()),
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
        let transport = self.draft_transport(cx);
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
            name: value(&self.name),
            transport,
            labels,
            public_ip,
        })
    }

    fn draft_transport(&self, cx: &Context<SystemView>) -> ManagedEndpointTransport {
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let address = value(&self.address);
        match self.transport {
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
        }
    }

    pub(super) fn provider_matches_draft(&self, cx: &Context<SystemView>) -> bool {
        match &self.target {
            EditorTarget::Saved(entry, _) => entry.config.transport == self.draft_transport(cx),
            EditorTarget::Add => match self.draft_transport(cx) {
                ManagedEndpointTransport::Unix { socket } => {
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
        self.environment_settings.editor = Some(Editor::new(target, window, cx));
        self.environment_settings.feedback = None;
        self.load_environment_vm(window, cx);
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (title, busy, body, footer) = owner
                .update(cx, |this, cx| {
                    let Some(editor) = &this.environment_settings.editor else {
                        return (gui::GUI_ENVIRONMENT_EDIT_TITLE, false, div(), div());
                    };
                    let screen = ScreenContext {
                        locale: appearance.locale,
                        palette: appearance.palette,
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
                        || (matches!(editor.target, EditorTarget::Builtin) && this.busy());
                    let height =
                        (window.viewport_size().height - px(218.)).clamp(px(165.), px(490.));
                    let body = this.environment_dialog_body(editor, &screen, height, cx);
                    let footer = this.environment_dialog_footer(editor, &screen, busy, cx);
                    (title, busy, body, footer)
                })
                .unwrap_or_else(|_| (gui::GUI_ENVIRONMENT_EDIT_TITLE, false, div(), div()));
            let locale = appearance.locale;
            let close = owner.clone();
            let close_button = owner.clone();
            let heading = row(locale)
                .w_full()
                .items_center()
                .justify_between()
                .gap_3()
                .child(title.text(locale))
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
            dialog
                .centered()
                .width(px(780.))
                .bg(color(appearance.palette, ColorToken::BgElevated))
                .border_color(color(appearance.palette, ColorToken::BorderSubtle))
                .title(heading)
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

    fn environment_dialog_body(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        height: marsa_ui::gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let builtin = matches!(editor.target, EditorTarget::Builtin);
        let mut sections = vec![Section::General];
        if builtin || (editor.vm.available() && editor.provider_matches_draft(cx)) {
            sections.extend([Section::Resources, Section::Network]);
        }
        if builtin || editor.workspace(cx).is_some() {
            sections.extend([Section::Activity, Section::Reports]);
        }
        let section = if sections.contains(&editor.section) {
            editor.section
        } else {
            Section::General
        };
        let tabs = row(locale)
            .gap_1()
            .flex_wrap()
            .p_1()
            .rounded(px(8.))
            .bg(color(screen.palette, ColorToken::BgSidebar))
            .children(sections.iter().copied().map(|tab_section| {
                Button::new(("environment-section", tab_section as usize))
                    .ghost()
                    .selected(tab_section == section)
                    .when(tab_section == section, |tab| {
                        tab.bg(color(screen.palette, ColorToken::AccentSubtle))
                    })
                    .label(tab_section.label().text(locale))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(editor) = &mut this.environment_settings.editor {
                            editor.section = tab_section;
                        }
                        if matches!(tab_section, Section::Activity | Section::Reports) {
                            this.load_environment_observations(
                                tab_section == Section::Reports,
                                window,
                                cx,
                            );
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
            Section::Activity => self.environment_activity(editor, screen, cx),
            Section::Reports => self.environment_report(editor, screen, cx),
        };
        let identity = match &editor.target {
            EditorTarget::Saved(entry, _) => Some(entry.config.name.as_str()),
            EditorTarget::Discovered(workspace) => Some(workspace.source.as_str()),
            EditorTarget::Builtin => Some("Marsa"),
            EditorTarget::Add => None,
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .children(identity.map(|name| {
                settings_text(screen, Role::Small, name.to_owned(), ColorToken::TextMuted)
            }))
            .children((sections.len() > 1).then_some(tabs))
            .child(
                div()
                    .id(("environment-section-body", editor.section as usize))
                    .h(height)
                    .overflow_y_scroll()
                    .child(page),
            )
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
        row(locale)
            .gap_2()
            .w_full()
            .flex_wrap()
            .children(
                (editable
                    || editor.target.is_discovered()
                    || matches!(editor.target, EditorTarget::Builtin))
                .then(|| {
                    Button::new("test-environment")
                        .outline()
                        .label(gui::GUI_ENVIRONMENT_TEST.text(locale))
                        .disabled(busy || self.services.is_none())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.environment_submit(true, window, cx)
                        }))
                }),
            )
            .child(div().flex_1())
            .child(
                Button::new("cancel-environment")
                    .outline()
                    .label(
                        if editable {
                            gui::GUI_DESKTOP_CANCEL
                        } else {
                            gui::GUI_DESKTOP_CLOSE
                        }
                        .text(locale),
                    )
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dismiss_environment_editor(cx);
                        window.close_dialog(cx);
                    })),
            )
            .children(editable.then(|| {
                Button::new("save-environment")
                    .primary()
                    .label(
                        if editor.id.is_some() {
                            gui::GUI_ENVIRONMENT_SAVE
                        } else {
                            gui::GUI_ENVIRONMENT_ADD
                        }
                        .text(locale),
                    )
                    .disabled(busy || self.services.is_none())
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.environment_submit(false, window, cx)
                        }),
                    )
            }))
    }

    fn environment_general(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        cx: &Context<Self>,
    ) -> Div {
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
                ))
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
                                editor.name.update(cx, |name, cx| {
                                    name.set_value(workspace.source.clone(), window, cx)
                                });
                                editor.address.update(cx, |address, cx| {
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
        let busy = self.environment_settings.task.is_some() || editor.vm.applying();
        let current = editor.transport;
        let owner = cx.weak_entity();
        let picker = Button::new("environment-transport")
            .outline()
            .icon(IconName::ChevronDown)
            .label(current.title(locale))
            .accessibility_label(gui::GUI_ENVIRONMENT_TRANSPORT.text(locale))
            .disabled(busy)
            .dropdown_menu(move |menu, _, _| {
                [Transport::Unix, Transport::Ssh, Transport::Tls]
                    .into_iter()
                    .fold(menu, |menu, kind| {
                        let owner = owner.clone();
                        menu.item(
                            PopupMenuItem::new(kind.title(locale))
                                .checked(kind == current)
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        if let Some(editor) = &mut this.environment_settings.editor
                                        {
                                            editor.transport = kind;
                                            this.environment_settings.feedback = None;
                                            cx.notify();
                                        }
                                    });
                                }),
                        )
                    })
            });
        let hint = match current {
            Transport::Unix => gui::GUI_ENVIRONMENT_UNIX_HELP,
            Transport::Ssh => gui::GUI_ENVIRONMENT_SSH_HELP,
            Transport::Tls => gui::GUI_ENVIRONMENT_TLS_HELP,
        };
        let mut form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(input_field(
                screen,
                gui::GUI_DESKTOP_NAME,
                &editor.name,
                busy,
                false,
                cx,
            ))
            .child(self.environment_labels(editor, screen, busy, cx))
            .child(
                row(locale)
                    .items_center()
                    .gap_3()
                    .child(settings_text(
                        screen,
                        Role::Label,
                        gui::GUI_ENVIRONMENT_TRANSPORT.text(locale),
                        ColorToken::TextPrimary,
                    ))
                    .child(picker),
            )
            .child(input_field(
                screen,
                current.address_label(),
                &editor.address,
                busy,
                false,
                cx,
            ))
            .child(settings_text(
                screen,
                Role::Small,
                hint.text(locale),
                ColorToken::TextMuted,
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
                form = form.child(
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
                                                editor.address.update(cx, |address, cx| {
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
                                                    editor.observations = Observations::default();
                                                    editor.observations_token =
                                                        std::rc::Rc::new(());
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
        if current == Transport::Tls {
            for (label, field) in [
                (gui::GUI_ENVIRONMENT_CA, &editor.ca),
                (gui::GUI_ENVIRONMENT_CERT, &editor.cert),
                (gui::GUI_ENVIRONMENT_KEY, &editor.key),
            ] {
                form = form.child(input_field(screen, label, field, busy, true, cx));
            }
        }
        form.child(input_field(
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
