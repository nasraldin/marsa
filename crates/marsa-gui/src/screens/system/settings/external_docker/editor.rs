//! Render and collect the saved connection form.

use std::path::PathBuf;

use marsa_ui::gpui::Subscription;

use marsa_client::docker::{ManagedEndpoint, ManagedEndpointConfig, ManagedEndpointTransport};
use marsa_ui::{
    ColorToken, Locale, TextKey,
    gpui::{
        AppContext as _, Context, Div, Entity, ParentElement as _, PathPromptOptions, Styled as _,
        Window, div, px,
    },
    gpui_component::{
        Disableable as _, IconName,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
    tokens::typography::Role,
};

use super::{SystemView, settings_text};
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
    transport: Transport,
    name: Entity<InputState>,
    address: Entity<InputState>,
    ca: Entity<InputState>,
    cert: Entity<InputState>,
    key: Entity<InputState>,
    _edit_observers: [Subscription; 5],
}

impl Editor {
    pub(super) fn new(
        existing: Option<&ManagedEndpoint>,
        window: &mut Window,
        cx: &mut Context<SystemView>,
    ) -> Self {
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
        let _edit_observers = [&name, &address, &ca, &cert, &key].map(|field| {
            cx.subscribe(field, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change)
                    && this.environment_settings.editor.is_some()
                    && this.environment_settings.feedback.take().is_some()
                {
                    cx.notify();
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
            _edit_observers,
        }
    }

    pub(super) fn config(&self, cx: &Context<SystemView>) -> ManagedEndpointConfig {
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        let address = value(&self.address);
        let transport = match self.transport {
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
        };
        ManagedEndpointConfig {
            name: value(&self.name),
            transport,
        }
    }
}

impl SystemView {
    pub(super) fn environment_editor(
        &self,
        editor: &Editor,
        screen: &ScreenContext<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = screen.locale;
        let busy = self.environment_settings.task.is_some();
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
        let title = if editor.id.is_some() {
            gui::GUI_ENVIRONMENT_EDIT_TITLE
        } else {
            gui::GUI_ENVIRONMENT_ADD
        };
        let hint = match current {
            Transport::Unix => gui::GUI_ENVIRONMENT_UNIX_HELP,
            Transport::Ssh => gui::GUI_ENVIRONMENT_SSH_HELP,
            Transport::Tls => gui::GUI_ENVIRONMENT_TLS_HELP,
        };
        let mut form = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded(px(8.))
            .bg(color(screen.palette, ColorToken::BgSidebar))
            .child(settings_text(
                screen,
                Role::Lead,
                title.text(locale),
                ColorToken::TextPrimary,
            ))
            .child(input_field(
                screen,
                gui::GUI_DESKTOP_NAME,
                &editor.name,
                busy,
                false,
                cx,
            ))
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
        if current == Transport::Tls {
            for (label, field) in [
                (gui::GUI_ENVIRONMENT_CA, &editor.ca),
                (gui::GUI_ENVIRONMENT_CERT, &editor.cert),
                (gui::GUI_ENVIRONMENT_KEY, &editor.key),
            ] {
                form = form.child(input_field(screen, label, field, busy, true, cx));
            }
        }
        form.child(
            row(locale)
                .gap_2()
                .flex_wrap()
                .child(
                    Button::new("test-environment")
                        .outline()
                        .label(gui::GUI_ENVIRONMENT_TEST.text(locale))
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.environment_submit(true, cx))),
                )
                .child(div().flex_1())
                .child(
                    Button::new("cancel-environment")
                        .ghost()
                        .label(gui::GUI_DESKTOP_CANCEL.text(locale))
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.environment_settings.editor = None;
                            this.environment_settings.feedback = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("save-environment")
                        .primary()
                        .label(gui::GUI_ENVIRONMENT_SAVE.text(locale))
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.environment_submit(false, cx))),
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
