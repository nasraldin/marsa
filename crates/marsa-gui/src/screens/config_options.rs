//! The same typed option editor powers reusable sets and individual container drafts.

use std::{cell::RefCell, rc::Rc};

use marsa_client::docker::container_config::{
    ConfigKeyValue, ConfigMount, ConfigMountKind, ConfigNetwork, ConfigPort, ConfigPortProtocol,
    ConfigRestartPolicy, ContainerConfigError, ContainerCreateOptions, MAX_CONFIG_MOUNTS,
    MAX_CONFIG_ROWS,
};
use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::{LogicalStyle as _, row},
    gpui::{App, AppContext as _, Div, Entity, ParentElement as _, Styled as _, Window, div},
    gpui_component::{
        Disableable as _, IconName,
        button::{Button, ButtonVariants as _},
        input::{InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
    },
    gui_keys as gui,
};

use super::common::{color, resource_input};

#[derive(Clone, Copy)]
enum Section {
    Environment,
    Labels,
    Ports,
    Mounts,
}

impl Section {
    const ALL: [Self; 4] = [Self::Environment, Self::Labels, Self::Ports, Self::Mounts];

    fn title(self, locale: Locale) -> &'static str {
        match self {
            Self::Environment => gui::GUI_CONFIG_ENVIRONMENT.text(locale),
            Self::Labels => gui::GUI_CONFIG_LABELS.text(locale),
            Self::Ports => gui::GUI_CONFIG_PORTS.text(locale),
            Self::Mounts => gui::GUI_CONFIG_MOUNTS.text(locale),
        }
    }

    fn labels(self, locale: Locale) -> [&'static str; 2] {
        let keys = match self {
            Self::Environment | Self::Labels => [gui::GUI_CONFIG_KEY, gui::GUI_CONFIG_VALUE],
            Self::Ports => [gui::GUI_CONFIG_HOST_PORT, gui::GUI_CONFIG_CONTAINER_PORT],
            Self::Mounts => [gui::GUI_CONFIG_MOUNT_SOURCE, gui::GUI_CONFIG_MOUNT_TARGET],
        };
        keys.map(|key| key.text(locale))
    }

    fn add_label(self, locale: Locale) -> &'static str {
        match self {
            Self::Environment => gui::GUI_CONFIG_ADD_ENVIRONMENT.text(locale),
            Self::Labels => gui::GUI_CONFIG_ADD_LABEL.text(locale),
            Self::Ports => gui::GUI_CONFIG_ADD_PORT.text(locale),
            Self::Mounts => gui::GUI_CONFIG_ADD_MOUNT.text(locale),
        }
    }

    fn limit(self) -> usize {
        match self {
            Self::Mounts => MAX_CONFIG_MOUNTS,
            _ => MAX_CONFIG_ROWS,
        }
    }

    fn default_choice(self) -> Extra {
        match self {
            Self::Ports => Extra::Protocol(ConfigPortProtocol::Tcp),
            Self::Mounts => Extra::Mount(ConfigMountKind::Volume, true),
            _ => Extra::Plain,
        }
    }
}

#[derive(Clone, Copy)]
enum Extra {
    Plain,
    Protocol(ConfigPortProtocol),
    Mount(ConfigMountKind, bool),
}

struct Entry {
    inputs: [Entity<InputState>; 2],
    extra: Extra,
}

impl Entry {
    fn new(
        values: [&str; 2],
        section: Section,
        extra: Extra,
        locale: Locale,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let labels = section.labels(locale);
        Self {
            inputs: std::array::from_fn(|index| field(labels[index], values[index], window, cx)),
            extra,
        }
    }

    fn values(&self, cx: &App) -> [String; 2] {
        self.inputs
            .each_ref()
            .map(|input| input.read(cx).value().to_string())
    }
}

pub(crate) type DraftOptions = Rc<RefCell<OptionsEditor>>;

pub(crate) struct OptionsEditor {
    network: ConfigNetwork,
    restart: ConfigRestartPolicy,
    entries: [Vec<Entry>; 4],
}

impl OptionsEditor {
    pub(crate) fn editable(
        options: &ContainerCreateOptions,
        locale: Locale,
        window: &mut Window,
        cx: &mut App,
    ) -> DraftOptions {
        let pairs = |items: &[ConfigKeyValue], section, window: &mut Window, cx: &mut App| {
            items
                .iter()
                .map(|pair| {
                    Entry::new(
                        [&pair.key, &pair.value],
                        section,
                        Extra::Plain,
                        locale,
                        window,
                        cx,
                    )
                })
                .collect()
        };
        let environment = pairs(&options.env, Section::Environment, window, cx);
        let labels = pairs(&options.labels, Section::Labels, window, cx);
        let ports = options
            .ports
            .iter()
            .map(|port| {
                Entry::new(
                    [&port.host.to_string(), &port.container.to_string()],
                    Section::Ports,
                    Extra::Protocol(port.protocol),
                    locale,
                    window,
                    cx,
                )
            })
            .collect();
        let mounts = options
            .mounts
            .iter()
            .map(|mount| {
                Entry::new(
                    [&mount.source, &mount.target],
                    Section::Mounts,
                    Extra::Mount(mount.kind, mount.read_only),
                    locale,
                    window,
                    cx,
                )
            })
            .collect();
        Rc::new(RefCell::new(Self {
            network: options.network,
            restart: options.restart,
            entries: [environment, labels, ports, mounts],
        }))
    }

    pub(crate) fn read(&self, cx: &App) -> Result<ContainerCreateOptions, ContainerConfigError> {
        let mut options = ContainerCreateOptions {
            network: self.network,
            restart: self.restart,
            ..ContainerCreateOptions::default()
        };
        for section in Section::ALL {
            for entry in &self.entries[section as usize] {
                let [first, second] = entry.values(cx);
                match entry.extra {
                    Extra::Plain => {
                        let pair = ConfigKeyValue {
                            key: first.trim().into(),
                            value: second,
                        };
                        match section {
                            Section::Environment => options.env.push(pair),
                            _ => options.labels.push(pair),
                        }
                    }
                    Extra::Protocol(protocol) => options.ports.push(ConfigPort {
                        host: first
                            .trim()
                            .parse()
                            .map_err(|_| ContainerConfigError::Ports)?,
                        container: second
                            .trim()
                            .parse()
                            .map_err(|_| ContainerConfigError::Ports)?,
                        protocol,
                    }),
                    Extra::Mount(kind, read_only) => options.mounts.push(ConfigMount {
                        kind,
                        source: first,
                        target: second,
                        read_only,
                    }),
                }
            }
        }
        options.validate()?;
        Ok(options)
    }

    pub(crate) fn render(draft: &DraftOptions, locale: Locale, palette: Palette, cx: &App) -> Div {
        let contents = draft.borrow();
        let network = draft.clone();
        let restart = draft.clone();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(caption(gui::GUI_CONFIG_NETWORK.text(locale), palette))
            .child(choice(
                "container-config-network",
                gui::GUI_CONFIG_NETWORK.text(locale),
                contents.network,
                [
                    (
                        ConfigNetwork::Default,
                        gui::GUI_CONFIG_NETWORK_DEFAULT.text(locale),
                    ),
                    (
                        ConfigNetwork::Bridge,
                        gui::GUI_CONFIG_NETWORK_BRIDGE.text(locale),
                    ),
                    (
                        ConfigNetwork::Disabled,
                        gui::GUI_CONFIG_NETWORK_NONE.text(locale),
                    ),
                ],
                move |value| network.borrow_mut().network = value,
            ))
            .child(caption(gui::GUI_CONFIG_RESTART.text(locale), palette))
            .child(choice(
                "container-config-restart",
                gui::GUI_CONFIG_RESTART.text(locale),
                contents.restart,
                [
                    (
                        ConfigRestartPolicy::Never,
                        gui::GUI_CONFIG_RESTART_NEVER.text(locale),
                    ),
                    (
                        ConfigRestartPolicy::OnFailure,
                        gui::GUI_CONFIG_RESTART_ON_FAILURE.text(locale),
                    ),
                    (
                        ConfigRestartPolicy::Always,
                        gui::GUI_CONFIG_RESTART_ALWAYS.text(locale),
                    ),
                    (
                        ConfigRestartPolicy::UnlessStopped,
                        gui::GUI_CONFIG_RESTART_UNLESS_STOPPED.text(locale),
                    ),
                ],
                move |value| restart.borrow_mut().restart = value,
            ))
            .children(Section::ALL.into_iter().map(|section| {
                let index = section as usize;
                let entries = &contents.entries[index];
                let add = draft.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_t_1()
                    .border_color(color(palette, ColorToken::BorderSubtle))
                    .pt_2()
                    .child(caption(section.title(locale), palette))
                    .children(matches!(section, Section::Ports).then(|| {
                        div()
                            .text_xs()
                            .text_color(color(palette, ColorToken::TextMuted))
                            .child(gui::GUI_CONFIG_PORT_HELP.text(locale))
                    }))
                    .children(
                        entries.iter().enumerate().map(|(slot, entry)| {
                            entry_row(entry, draft, section, slot, locale, cx)
                        }),
                    )
                    .child(
                        Button::new(("add-config-row", index))
                            .outline()
                            .label(section.add_label(locale))
                            .disabled(entries.len() >= section.limit())
                            .on_click(move |_, window, cx| {
                                let mut add = add.borrow_mut();
                                if add.entries[index].len() < section.limit() {
                                    add.entries[index].push(Entry::new(
                                        ["", ""],
                                        section,
                                        section.default_choice(),
                                        locale,
                                        window,
                                        cx,
                                    ));
                                    window.refresh();
                                }
                            }),
                    )
            }))
    }
}

fn entry_row(
    entry: &Entry,
    draft: &DraftOptions,
    section: Section,
    slot: usize,
    locale: Locale,
    cx: &App,
) -> Div {
    let index = section as usize;
    let id = index * MAX_CONFIG_ROWS + slot;
    let remove = draft.clone();
    let mut controls = row(cx).gap_2().flex_wrap().items_center();
    match entry.extra {
        Extra::Plain => {}
        Extra::Protocol(protocol) => {
            let draft = draft.clone();
            controls = controls.child(choice(
                ("config-port-protocol", id),
                gui::GUI_CONFIG_PROTOCOL.text(locale),
                protocol,
                [
                    (ConfigPortProtocol::Tcp, gui::GUI_CONFIG_TCP.text(locale)),
                    (ConfigPortProtocol::Udp, gui::GUI_CONFIG_UDP.text(locale)),
                ],
                move |protocol| {
                    if let Some(entry) = draft.borrow_mut().entries[index].get_mut(slot) {
                        entry.extra = Extra::Protocol(protocol);
                    }
                },
            ));
        }
        Extra::Mount(kind, read_only) => {
            let source = draft.clone();
            let access = draft.clone();
            controls = controls
                .child(choice(
                    ("config-mount-kind", id),
                    gui::GUI_CONFIG_MOUNT_KIND.text(locale),
                    kind,
                    [
                        (ConfigMountKind::Volume, gui::GUI_CONFIG_VOLUME.text(locale)),
                        (ConfigMountKind::Host, gui::GUI_CONFIG_HOST.text(locale)),
                    ],
                    move |kind| {
                        if let Some(entry) = source.borrow_mut().entries[index].get_mut(slot)
                            && let Extra::Mount(_, read_only) = entry.extra
                        {
                            entry.extra = Extra::Mount(kind, read_only);
                        }
                    },
                ))
                .child(choice(
                    ("config-mount-access", id),
                    gui::GUI_CONFIG_MOUNT_ACCESS.text(locale),
                    read_only,
                    [
                        (true, gui::GUI_CONFIG_READ_ONLY.text(locale)),
                        (false, gui::GUI_CONFIG_READ_WRITE.text(locale)),
                    ],
                    move |read_only| {
                        if let Some(entry) = access.borrow_mut().entries[index].get_mut(slot)
                            && let Extra::Mount(kind, _) = entry.extra
                        {
                            entry.extra = Extra::Mount(kind, read_only);
                        }
                    },
                ));
        }
    }
    controls = controls.child(
        Button::new(("remove-config-row", id))
            .ghost()
            .label(gui::GUI_CONFIG_REMOVE_ROW.text(locale))
            .on_click(move |_, window, _| {
                let mut remove = remove.borrow_mut();
                if slot < remove.entries[index].len() {
                    remove.entries[index].remove(slot);
                    window.refresh();
                }
            }),
    );
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            row(cx).gap_2().children(
                entry
                    .inputs
                    .iter()
                    .map(|input| div().min_w_0().flex_1().child(resource_input(input))),
            ),
        )
        .child(controls)
}

fn choice<T: Copy + PartialEq + 'static, const N: usize>(
    id: impl Into<marsa_ui::gpui::ElementId>,
    title: &'static str,
    selected: T,
    choices: [(T, &'static str); N],
    change: impl Fn(T) + 'static,
) -> impl marsa_ui::gpui::IntoElement {
    let value = choices
        .iter()
        .find(|(choice, _)| *choice == selected)
        .map_or(title, |(_, label)| label);
    let change = Rc::new(change);
    Button::new(id)
        .outline()
        .icon(IconName::ChevronDown)
        .label(value)
        .accessibility_label(title)
        .dropdown_menu(move |menu, _, _| {
            choices.into_iter().fold(menu, |menu, (choice, label)| {
                let change = change.clone();
                menu.item(
                    PopupMenuItem::new(label)
                        .checked(choice == selected)
                        .on_click(move |_, window, _| {
                            change(choice);
                            window.refresh();
                        }),
                )
            })
        })
}

pub(crate) fn field(
    placeholder: &'static str,
    value: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    if !value.is_empty() {
        input.update(cx, |input, cx| {
            input.set_value(value.to_owned(), window, cx)
        });
    }
    cx.subscribe(&input, |_, _: &InputEvent, cx| cx.refresh_windows())
        .detach();
    input
}

fn caption(text: &'static str, palette: Palette) -> Div {
    div()
        .text_sm()
        .text_color(color(palette, ColorToken::TextPrimary))
        .child(text)
}

pub(crate) fn error_text(error: ContainerConfigError, locale: Locale) -> &'static str {
    match error {
        ContainerConfigError::Details => gui::GUI_CONFIG_INVALID_DETAILS.text(locale),
        ContainerConfigError::Capacity => gui::GUI_CONFIG_TOO_MANY.text(locale),
        ContainerConfigError::Environment => gui::GUI_CONFIG_INVALID_ENVIRONMENT.text(locale),
        ContainerConfigError::Labels => gui::GUI_CONFIG_INVALID_LABELS.text(locale),
        ContainerConfigError::Ports => gui::GUI_CONFIG_INVALID_PORTS.text(locale),
        ContainerConfigError::Mounts => gui::GUI_CONFIG_INVALID_MOUNTS.text(locale),
    }
}

pub(crate) fn error_line(text: &'static str, palette: Palette, cx: &App) -> Div {
    div()
        .text_start(cx)
        .text_sm()
        .text_color(color(palette, ColorToken::StatusError))
        .child(text)
}
