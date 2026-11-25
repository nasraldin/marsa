//! Multi-step Pull image dialog: Configure → Pull layers → Keep or Remove.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Instant,
};

use marsa_client::docker::{
    DockerError, ExternalEndpoint, ImageId, ImageReference, ImageRemoval, PullProgress,
    RegistryAddress, RegistryStore, RemovalMode,
};
use marsa_ui::{
    ColorToken, Locale, Palette,
    direction::isolate_technical_text,
    gpui::{
        App, AppContext, Context, Div, Entity, FontWeight, IntoElement, ParentElement as _,
        SharedString, Styled as _, Window, div, px, relative,
    },
    gpui_component::{
        Disableable as _, Icon, IconName, WindowExt as _,
        button::{Button, ButtonVariants as _},
        input::InputState,
        menu::{DropdownMenu as _, PopupMenuItem},
        scroll::ScrollableElement as _,
    },
    gui_keys as gui,
    tokens::{
        scale::{Radius, Space},
        typography::{Role, ui_size},
    },
};

use crate::{
    screens::common::{
        ScreenContext, color, dialog_actions, dialog_frame, message, resource_input, row,
    },
    services::{ServiceError, Services},
};

use super::{ResourceKind, ResourceSource, ResourcesView, operations::Outcome, text, tone};

const MAX_LAYERS: usize = 256;
const MAX_LABEL_BYTES: usize = 512;
const MAX_LOG_LINES: usize = 200;

/// One registry choice in the Configure dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RegistryChoice {
    /// Authority used when composing the full reference (`docker.io` for Hub).
    pub authority: String,
    /// Localized or display label for the menu.
    pub label: SharedString,
    /// Optional brand mark asset path.
    pub icon: Option<&'static str>,
}

impl RegistryChoice {
    fn docker_hub(locale: Locale) -> Self {
        Self {
            authority: RegistryAddress::docker_hub().as_str().to_owned(),
            label: message(locale, gui::GUI_DESKTOP_DOCKER_HUB_PUBLIC),
            icon: Some("marsa/docker.svg"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PullStep {
    Configure,
    Pull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerKind {
    Pending,
    Active,
    Done,
}

struct LayerRow {
    status: String,
    kind: LayerKind,
    current: Option<u64>,
    total: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogKind {
    Pull,
    Layer,
}

struct LogLine {
    kind: LogKind,
    text: String,
}

/// Session state for the Pull image wizard dialog.
pub(super) struct PullState {
    step: PullStep,
    environment: SharedString,
    endpoint: Option<ExternalEndpoint>,
    registries: Vec<RegistryChoice>,
    registry_index: usize,
    input: Entity<InputState>,
    reference: String,
    layers: BTreeMap<String, LayerRow>,
    logs: VecDeque<LogLine>,
    started: Option<Instant>,
    finished: Option<std::time::Duration>,
    outcome: Option<Outcome>,
    image_id: Option<String>,
    output_open: bool,
    active: bool,
}

impl ResourcesView {
    /// Open the Configure → Pull wizard for the pinned environment.
    pub(super) fn open_pull(
        &mut self,
        source: Option<ResourceSource>,
        appearance: (Locale, Palette, marsa_ui::preferences::DisplaySettings),
        service: Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() || self.pull.is_some() {
            return;
        }
        let (locale, palette, display) = appearance;
        let environment = source.as_ref().map_or_else(
            || SharedString::from("Marsa"),
            |source| source.badge.clone(),
        );
        let endpoint = source.as_ref().map(|source| source.endpoint.clone());
        let state = source.as_ref().map_or_else(
            || service.store().select(Clone::clone).current().clone(),
            |source| source.state.clone(),
        );
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(gui::GUI_DESKTOP_PULL_IMAGE_PLACEHOLDER.text(locale))
        });
        cx.subscribe(
            &input,
            |_, _, _: &marsa_ui::gpui_component::input::InputEvent, cx| cx.refresh_windows(),
        )
        .detach();
        self.pull = Some(PullState {
            step: PullStep::Configure,
            environment,
            endpoint,
            registries: vec![RegistryChoice::docker_hub(locale)],
            registry_index: 0,
            input,
            reference: String::new(),
            layers: BTreeMap::new(),
            logs: VecDeque::new(),
            started: None,
            finished: None,
            outcome: None,
            image_id: None,
            output_open: true,
            active: false,
        });
        let view = cx.entity();
        // RegistryStore talks to Tokio (fs/helper). Run it on the client runtime,
        // not the GPUI local executor — Handle::current() panics there.
        let mut registries = service.pending(async move { Ok(load_registries(locale).await) });
        self.operation = Some(cx.spawn(async move |view, cx| {
            let Some(Ok(registries)) = registries.result().await else {
                return;
            };
            let _ = view.update(cx, |view, cx| {
                if let Some(pull) = &mut view.pull
                    && pull.step == PullStep::Configure
                {
                    let selected = pull
                        .registries
                        .get(pull.registry_index)
                        .map(|choice| choice.authority.clone());
                    pull.registries = registries;
                    if let Some(authority) = selected {
                        pull.registry_index = pull
                            .registries
                            .iter()
                            .position(|choice| choice.authority == authority)
                            .unwrap_or(0);
                    }
                    cx.refresh_windows();
                }
            });
        }));
        window.open_dialog(cx, move |dialog, _, cx| {
            let screen = ScreenContext {
                locale,
                palette,
                display,
                state: &state,
                services: Some(&service),
            };
            let Some(pull) = view.read(cx).pull.as_ref() else {
                return dialog_frame(dialog, 640.);
            };
            let (title, body, footer) = render_dialog(&screen, &view, pull, cx);
            dialog_frame(dialog, 640.)
                .overlay_closable(false)
                .title(title)
                .on_ok(|_, _, _| false)
                .child(body)
                .footer(footer)
        });
    }

    pub(super) fn set_pull_registry(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(pull) = &mut self.pull else {
            return;
        };
        if index < pull.registries.len() {
            pull.registry_index = index;
            cx.refresh_windows();
        }
    }

    pub(super) fn toggle_pull_output(&mut self, cx: &mut Context<Self>) {
        if let Some(pull) = &mut self.pull {
            pull.output_open = !pull.output_open;
            cx.refresh_windows();
        }
    }

    pub(super) fn close_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pull.as_ref().is_some_and(|pull| pull.active) {
            self.operation = None;
            self.busy = None;
        }
        self.pull = None;
        window.close_dialog(cx);
        cx.refresh_windows();
    }

    pub(super) fn keep_pulled_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pull = None;
        self.busy = None;
        self.outcome = Some((
            ResourceKind::Images,
            Outcome::Success(super::operations::SuccessKind::ImagePulled),
        ));
        window.close_dialog(cx);
        cx.refresh_windows();
    }

    pub(super) fn remove_pulled_image(
        &mut self,
        service: Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pull) = self.pull.take() else {
            window.close_dialog(cx);
            return;
        };
        let endpoint = pull.endpoint.clone();
        let image_id = pull.image_id.clone();
        let reference = pull.reference.clone();
        window.close_dialog(cx);
        self.busy = Some(ResourceKind::Images);
        self.outcome = None;
        let external = endpoint.is_some();
        self.operation = Some(cx.spawn(async move |view, cx| {
            let mut outcome = Outcome::Rejected;
            if let Some(id) = image_id.as_deref().or(Some(reference.as_str())) {
                let docker = service.resource_docker(endpoint.clone());
                let removal = ImageId::parse(id.to_owned())
                    .ok()
                    .map(|id| ImageRemoval::confirmed(id, RemovalMode::StoppedOnly));
                if let Some(removal) = removal {
                    if matches!(
                        service
                            .pending(async move { Ok(docker.remove_image(removal).await?) })
                            .result()
                            .await,
                        Some(Ok(()))
                    ) {
                        outcome = Outcome::Success(super::operations::SuccessKind::ImageRemoved);
                    }
                } else {
                    // Tag-based remove: list then match by repo tag.
                    let docker = service.resource_docker(endpoint.clone());
                    let reference = reference.clone();
                    let found = service
                        .pending(async move {
                            let images = docker.images().await?;
                            Ok(images.into_iter().find_map(|image| {
                                image
                                    .repo_tags
                                    .iter()
                                    .any(|tag| tag == &reference)
                                    .then_some(image.id)
                            }))
                        })
                        .result()
                        .await;
                    if let Some(Ok(Some(id))) = found
                        && let Ok(id) = ImageId::parse(id)
                    {
                        let docker = service.resource_docker(endpoint.clone());
                        let removal = ImageRemoval::confirmed(id, RemovalMode::StoppedOnly);
                        if matches!(
                            service
                                .pending(async move { Ok(docker.remove_image(removal).await?) })
                                .result()
                                .await,
                            Some(Ok(()))
                        ) {
                            outcome =
                                Outcome::Success(super::operations::SuccessKind::ImageRemoved);
                        }
                    }
                }
            }
            if !external && outcome.is_success() {
                let _ = service.refresh().result().await;
            }
            let _ = view.update(cx, |view, cx| {
                view.busy = None;
                if external {
                    cx.emit(super::ExternalResourcesChanged);
                }
                view.outcome = Some((ResourceKind::Images, outcome));
                cx.refresh_windows();
            });
        }));
        cx.refresh_windows();
    }

    /// Start the engine pull for the composed reference and switch to the Pull step.
    pub(super) fn start_pull(&mut self, service: Arc<Services>, cx: &mut Context<Self>) {
        let Some(pull) = &mut self.pull else {
            return;
        };
        if pull.active || pull.step != PullStep::Configure {
            return;
        }
        let name = pull.input.read(cx).value();
        let authority = pull
            .registries
            .get(pull.registry_index)
            .map(|choice| choice.authority.as_str())
            .unwrap_or("docker.io");
        let reference = compose_reference(authority, name.trim());
        let Ok(request) = ImageReference::parse(reference.clone()) else {
            pull.outcome = Some(Outcome::Rejected);
            cx.refresh_windows();
            return;
        };
        pull.reference = reference.clone();
        pull.step = PullStep::Pull;
        pull.layers.clear();
        pull.logs.clear();
        pull.started = Some(Instant::now());
        pull.finished = None;
        pull.outcome = None;
        pull.image_id = None;
        pull.active = true;
        push_log(
            &mut pull.logs,
            LogKind::Pull,
            format!("Starting pull for {reference}"),
        );
        self.busy = Some(ResourceKind::Images);
        self.outcome = None;
        let endpoint = pull.endpoint.clone();
        let external = endpoint.is_some();
        let mut stream = service.pull_image(request, endpoint);
        self.operation = Some(cx.spawn(async move |view, cx| {
            let mut outcome = Outcome::Success(super::operations::SuccessKind::ImagePulled);
            while let Some(batch) = stream.next().await {
                let records = match batch {
                    Ok(records) => records,
                    Err(ServiceError::Docker(DockerError::ImagePlatform)) => {
                        outcome = Outcome::UnsupportedImagePlatform;
                        break;
                    }
                    Err(_) => {
                        outcome = Outcome::Rejected;
                        break;
                    }
                };
                let update = view.update(cx, |view, cx| {
                    let Some(pull) = &mut view.pull else {
                        return;
                    };
                    for record in records {
                        apply_progress(pull, record);
                    }
                    cx.refresh_windows();
                });
                if update.is_err() {
                    return;
                }
            }
            if !external && outcome.is_success() {
                let _ = service.refresh().result().await;
            }
            let _ = view.update(cx, |view, cx| {
                if let Some(pull) = &mut view.pull {
                    pull.active = false;
                    if let Some(started) = pull.started {
                        pull.finished = Some(started.elapsed());
                    }
                    pull.outcome = Some(outcome);
                    if outcome.is_success() {
                        let duration = pull
                            .finished
                            .map(|value| format!("{:.1}s", value.as_secs_f64()))
                            .unwrap_or_else(|| "0s".into());
                        push_log(
                            &mut pull.logs,
                            LogKind::Pull,
                            format!("Pull completed in {duration}"),
                        );
                    } else {
                        push_log(
                            &mut pull.logs,
                            LogKind::Pull,
                            match outcome {
                                Outcome::UnsupportedImagePlatform => {
                                    "Pull failed: unsupported image platform".into()
                                }
                                Outcome::Cancelled => "Pull cancelled".into(),
                                _ => "Pull failed".into(),
                            },
                        );
                    }
                }
                view.busy = None;
                if external {
                    cx.emit(super::ExternalResourcesChanged);
                }
                // Toast only on failure here; Keep / Remove report success.
                if !outcome.is_success() {
                    view.outcome = Some((ResourceKind::Images, outcome));
                }
                cx.refresh_windows();
            });
        }));
        cx.refresh_windows();
    }

    fn cancel_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pull) = &mut self.pull else {
            return;
        };
        if pull.active {
            self.operation = None;
            pull.active = false;
            pull.outcome = Some(Outcome::Cancelled);
            if let Some(started) = pull.started {
                pull.finished = Some(started.elapsed());
            }
            push_log(&mut pull.logs, LogKind::Pull, "Pull cancelled".into());
            self.busy = None;
            self.outcome = Some((ResourceKind::Images, Outcome::Cancelled));
            cx.refresh_windows();
            return;
        }
        self.close_pull(window, cx);
    }
}

async fn load_registries(locale: Locale) -> Vec<RegistryChoice> {
    let mut registries = vec![RegistryChoice::docker_hub(locale)];
    if let Ok(store) = RegistryStore::discover().await
        && let Ok(entries) = store.list().await
    {
        for entry in entries {
            let authority = entry.address().as_str().to_owned();
            if registries
                .iter()
                .any(|choice| choice.authority == authority)
            {
                continue;
            }
            registries.push(RegistryChoice {
                label: SharedString::from(format!(
                    "{} ({})",
                    isolate_technical_text(&authority),
                    entry.username()
                )),
                authority,
                icon: None,
            });
        }
    }
    registries
}

/// Compose a Docker image reference from registry authority and typed name.
pub(super) fn compose_reference(authority: &str, name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let with_tag = if trimmed.contains(':') || trimmed.contains('@') {
        trimmed.to_owned()
    } else {
        format!("{trimmed}:latest")
    };
    if authority.is_empty() || authority == "docker.io" {
        return with_tag;
    }
    if with_tag.starts_with(&format!("{authority}/")) || with_tag.starts_with(authority) {
        with_tag
    } else {
        format!("{authority}/{with_tag}")
    }
}

fn apply_progress(pull: &mut PullState, mut record: PullProgress) {
    if let Some(status) = &mut record.status {
        status.truncate(status.floor_char_boundary(MAX_LABEL_BYTES.min(status.len())));
    }
    let status = record.status.clone().unwrap_or_default();
    if let Some(digest) = status.strip_prefix("Digest: ") {
        let digest = digest.trim();
        if digest.starts_with("sha256:") && digest.len() == 71 {
            pull.image_id = Some(digest.to_owned());
        }
        push_log(&mut pull.logs, LogKind::Pull, status.clone());
        return;
    }
    let id = record.id.as_deref().unwrap_or_default();
    if id.is_empty() {
        if !status.is_empty() {
            push_log(&mut pull.logs, LogKind::Pull, status);
        }
        return;
    }
    if id.len() > MAX_LABEL_BYTES {
        return;
    }
    if pull.layers.len() >= MAX_LAYERS && !pull.layers.contains_key(id) {
        return;
    }
    let kind = layer_kind(&status, record.progress.as_ref());
    let current = record.progress.as_ref().and_then(|p| p.current);
    let total = record.progress.as_ref().and_then(|p| p.total);
    push_log(&mut pull.logs, LogKind::Layer, format!("{id}: {status}"));
    pull.layers.insert(
        id.to_owned(),
        LayerRow {
            status,
            kind,
            current,
            total,
        },
    );
}

fn layer_kind(status: &str, progress: Option<&marsa_client::docker::PullCounters>) -> LayerKind {
    let lower = status.to_ascii_lowercase();
    if lower.contains("complete")
        || lower.contains("already exists")
        || lower.contains("pull complete")
        || lower == "done"
    {
        return LayerKind::Done;
    }
    if progress.is_some_and(|p| p.current.is_some())
        || lower.contains("downloading")
        || lower.contains("extracting")
        || lower.contains("verifying")
    {
        return LayerKind::Active;
    }
    LayerKind::Pending
}

fn push_log(logs: &mut VecDeque<LogLine>, kind: LogKind, text: String) {
    if text.is_empty() {
        return;
    }
    logs.push_back(LogLine { kind, text });
    while logs.len() > MAX_LOG_LINES {
        logs.pop_front();
    }
}

fn render_dialog(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    pull: &PullState,
    cx: &App,
) -> (Div, Div, Div) {
    let completed = pull.outcome.is_some_and(Outcome::is_success);
    let failed = pull
        .outcome
        .is_some_and(|outcome| !outcome.is_success() && !matches!(outcome, Outcome::Cancelled));
    let title = pull_title(screen, view, pull, completed);
    let body = div()
        .flex()
        .flex_col()
        .gap(px(Space::S16.pixels()))
        .max_h(px(520.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(Space::S16.pixels()))
                .overflow_y_scrollbar()
                .child(stepper(screen, pull, completed))
                .child(match pull.step {
                    PullStep::Configure => configure_body(screen, view, pull, cx),
                    PullStep::Pull => pull_body(screen, view, pull, completed, failed),
                }),
        );
    let footer = match pull.step {
        PullStep::Configure => configure_footer(screen, view, pull, cx),
        PullStep::Pull if completed => complete_footer(screen, view),
        PullStep::Pull => pulling_footer(screen, view, pull.active),
    };
    (title, body, footer)
}

fn pull_title(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    pull: &PullState,
    completed: bool,
) -> Div {
    let locale = screen.locale;
    let env = pull.environment.clone();
    let prefix = message(locale, gui::GUI_DESKTOP_PULL_IMAGE_TO);
    row(locale)
        .w_full()
        .items_start()
        .gap_3()
        .pb_3()
        .border_b_1()
        .border_color(color(screen.palette, ColorToken::BorderSubtle))
        .child(
            row(locale)
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .flex_wrap()
                .children(completed.then(|| {
                    Icon::new(IconName::CircleCheck)
                        .text_color(color(screen.palette, ColorToken::StatusRunning))
                        .into_any_element()
                }))
                .children((!completed).then(|| {
                    Icon::default()
                        .path("icons/download.svg")
                        .text_color(color(screen.palette, ColorToken::TextPrimary))
                        .into_any_element()
                }))
                .child(
                    div()
                        .text_size(ui_size(21.))
                        .line_height(ui_size(28.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(screen.palette, ColorToken::TextPrimary))
                        .child(
                            row(locale).items_baseline().gap_1().child(prefix).child(
                                div()
                                    .text_color(color(screen.palette, ColorToken::AccentDefault))
                                    .child(env),
                            ),
                        ),
                )
                .children((!pull.reference.is_empty()).then(|| {
                    div()
                        .rounded(px(Radius::Full.pixels()))
                        .px(px(Space::S8.pixels()))
                        .py(px(Space::S4.pixels()))
                        .bg(color(screen.palette, ColorToken::BgCode))
                        .font_family("IBM Plex Mono")
                        .text_sm()
                        .text_color(color(screen.palette, ColorToken::TextMuted))
                        .child(isolate_technical_text(&pull.reference))
                })),
        )
        .child(view_close_button(screen, view))
}

fn view_close_button(screen: &ScreenContext<'_>, view: &Entity<ResourcesView>) -> impl IntoElement {
    let view = view.downgrade();
    let locale = screen.locale;
    Button::new("close-pull-dialog")
        .ghost()
        .size(px(30.))
        .flex_shrink_0()
        .rounded(px(6.))
        .icon(IconName::Close)
        .accessibility_label(gui::GUI_DESKTOP_CLOSE.text(locale))
        .on_click(move |_, window, cx| {
            let _ = view.update(cx, |view, cx| view.close_pull(window, cx));
        })
}

fn stepper(screen: &ScreenContext<'_>, pull: &PullState, completed: bool) -> Div {
    let locale = screen.locale;
    let configure_active = pull.step == PullStep::Configure;
    let pull_active = pull.step == PullStep::Pull;
    row(locale)
        .w_full()
        .items_center()
        .gap_3()
        .child(step_tab(
            screen,
            "pull-step-configure",
            "icons/sliders-horizontal.svg",
            message(locale, gui::GUI_DESKTOP_PULL_STEP_CONFIGURE),
            configure_active,
            false,
        ))
        .child(
            Icon::new(IconName::ArrowRight)
                .text_color(color(screen.palette, ColorToken::TextMuted))
                .into_any_element(),
        )
        .child(step_tab(
            screen,
            "pull-step-pull",
            "icons/download.svg",
            message(locale, gui::GUI_DESKTOP_PULL_STEP_PULL),
            pull_active,
            completed,
        ))
}

fn step_tab(
    screen: &ScreenContext<'_>,
    id: &'static str,
    icon: &'static str,
    label: SharedString,
    active: bool,
    done: bool,
) -> Div {
    let tone = if active || done {
        ColorToken::AccentDefault
    } else {
        ColorToken::TextMuted
    };
    let _ = id;
    let tab = row(screen.locale)
        .items_center()
        .gap_2()
        .pb_2()
        .child(if done {
            Icon::new(IconName::CircleCheck)
                .text_color(color(screen.palette, tone))
                .into_any_element()
        } else {
            Icon::default()
                .path(icon)
                .text_color(color(screen.palette, tone))
                .into_any_element()
        })
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(screen.palette, tone))
                .child(label),
        );
    if active {
        tab.border_b_2()
            .border_color(color(screen.palette, ColorToken::AccentDefault))
    } else {
        tab
    }
}

fn configure_body(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    pull: &PullState,
    cx: &App,
) -> Div {
    let locale = screen.locale;
    let selected = pull
        .registries
        .get(pull.registry_index)
        .cloned()
        .unwrap_or_else(|| RegistryChoice::docker_hub(locale));
    let name = pull.input.read(cx).value();
    let preview = compose_reference(&selected.authority, name.trim());
    let view_for_menu = view.downgrade();
    let registries = pull.registries.clone();
    let selected_index = pull.registry_index;
    // Dialog is 640px with 24px padding; match the full-width trigger.
    const REGISTRY_MENU_MIN_W: f32 = 592.;
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S16.pixels()))
        .child(field(
            screen,
            message(locale, gui::GUI_DESKTOP_REGISTRY),
            Button::new("pull-registry")
                .outline()
                .w_full()
                .h(px(Space::S36.pixels()))
                .dropdown_caret(true)
                // One flex child + caret → justify_between pins label to start (not center).
                .child(
                    row(locale)
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(match selected.icon {
                            Some(path) => Icon::default().path(path).into_any_element(),
                            None => Icon::new(IconName::Globe).into_any_element(),
                        })
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .whitespace_nowrap()
                                .child(selected.label.clone()),
                        ),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu.min_w(px(REGISTRY_MENU_MIN_W));
                    for (index, choice) in registries.iter().enumerate() {
                        let view = view_for_menu.clone();
                        let label = choice.label.clone();
                        let icon = match choice.icon {
                            Some(path) => Icon::default().path(path),
                            None => Icon::new(IconName::Globe),
                        };
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .icon(icon)
                                .checked(index == selected_index)
                                .on_click(move |_, _, cx| {
                                    let _ = view.update(cx, |view, cx| {
                                        view.set_pull_registry(index, cx);
                                    });
                                }),
                        );
                    }
                    menu
                })
                .into_any_element(),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(Space::S4.pixels()))
                .child(field(
                    screen,
                    message(locale, gui::GUI_DESKTOP_IMAGE_NAME),
                    resource_input(&pull.input)
                        .aria_label(gui::GUI_DESKTOP_IMAGE_NAME.text(locale))
                        .into_any_element(),
                ))
                .child(
                    text(
                        screen,
                        Role::Small,
                        message(locale, gui::GUI_DESKTOP_PULL_IMAGE_FORMAT_HELP),
                    )
                    .text_color(tone(screen, ColorToken::TextMuted)),
                ),
        )
        .children((!preview.is_empty()).then(|| {
            field(
                screen,
                message(locale, gui::GUI_DESKTOP_FULL_IMAGE_REFERENCE),
                div()
                    .w_full()
                    .h(px(Space::S36.pixels()))
                    .px(px(Space::S12.pixels()))
                    .rounded(px(Radius::R8.pixels()))
                    .border_1()
                    .border_color(tone(screen, ColorToken::BorderSubtle))
                    .bg(tone(screen, ColorToken::BgCode))
                    .flex()
                    .items_center()
                    .font_family("IBM Plex Mono")
                    .text_sm()
                    .text_color(tone(screen, ColorToken::TextPrimary))
                    .child(isolate_technical_text(&preview))
                    .into_any_element(),
            )
        }))
}

fn field(screen: &ScreenContext<'_>, label: SharedString, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S4.pixels()))
        .child(text(screen, Role::Small, label).font_weight(FontWeight::MEDIUM))
        .child(control)
}

fn pull_body(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    pull: &PullState,
    completed: bool,
    failed: bool,
) -> Div {
    let locale = screen.locale;
    let (done, total) = layer_counts(pull);
    let (downloaded, expected) = byte_totals(pull);
    let status_label = if completed {
        message(locale, gui::GUI_DESKTOP_PULL_COMPLETED)
    } else if failed {
        message(locale, gui::GUI_DESKTOP_PULL_FAILED)
    } else {
        message(locale, gui::GUI_DESKTOP_PULLING_LAYERS)
    };
    let status_color = if completed {
        ColorToken::StatusRunning
    } else if failed {
        ColorToken::StatusError
    } else {
        ColorToken::AccentDefault
    };
    let progress = if expected > 0 {
        (downloaded as f32 / expected as f32).clamp(0., 1.)
    } else if total > 0 {
        done as f32 / total as f32
    } else {
        0.
    };
    div()
        .flex()
        .flex_col()
        .gap(px(Space::S12.pixels()))
        .child(
            row(locale)
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    row(locale)
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(if completed {
                                IconName::CircleCheck
                            } else if failed {
                                IconName::CircleAlert
                            } else {
                                IconName::LoaderCircle
                            })
                            .text_color(color(screen.palette, status_color)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color(screen.palette, status_color))
                                .child(status_label),
                        ),
                )
                .child(
                    row(locale)
                        .items_center()
                        .gap_2()
                        .child(
                            text(
                                screen,
                                Role::Small,
                                SharedString::from(format!(
                                    "{done} / {total} {}",
                                    gui::GUI_DESKTOP_LAYERS.text(locale)
                                )),
                            )
                            .text_color(tone(screen, ColorToken::TextMuted)),
                        )
                        .children(pull.finished.map(|duration| {
                            text(
                                screen,
                                Role::Small,
                                SharedString::from(format!("{:.1}s", duration.as_secs_f64())),
                            )
                            .text_color(tone(screen, ColorToken::TextMuted))
                        })),
                ),
        )
        .children((!completed).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(Space::S4.pixels()))
                .child(progress_bar(screen, progress))
                .child(
                    text(
                        screen,
                        Role::Small,
                        SharedString::from(format!(
                            "{}: {} / {}",
                            gui::GUI_DESKTOP_DOWNLOADED.text(locale),
                            format_bytes(locale, downloaded),
                            format_bytes(locale, expected)
                        )),
                    )
                    .text_color(tone(screen, ColorToken::TextMuted)),
                )
        }))
        .child(layers_table(screen, pull))
        .child(output_panel(screen, view, pull))
}

fn progress_bar(screen: &ScreenContext<'_>, fraction: f32) -> Div {
    div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(color(screen.palette, ColorToken::BgCode))
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(color(screen.palette, ColorToken::AccentDefault))
                .w(relative(fraction.clamp(0., 1.))),
        )
}

fn layers_table(screen: &ScreenContext<'_>, pull: &PullState) -> Div {
    let locale = screen.locale;
    let header = row(locale)
        .w_full()
        .items_center()
        .px(px(Space::S8.pixels()))
        .py(px(Space::S4.pixels()))
        .border_b_1()
        .border_color(tone(screen, ColorToken::BorderSubtle))
        .child(header_cell(screen, gui::GUI_DESKTOP_LAYER_ID, 1.2))
        .child(header_cell(screen, gui::GUI_DESKTOP_STATUS, 2.0))
        .child(header_cell(screen, gui::GUI_DESKTOP_PROGRESS, 1.2));
    let rows = div()
        .flex()
        .flex_col()
        .children(pull.layers.iter().map(|(id, layer)| {
            let color_token = match layer.kind {
                LayerKind::Done => ColorToken::StatusRunning,
                LayerKind::Active => ColorToken::AccentDefault,
                LayerKind::Pending => ColorToken::TextMuted,
            };
            let status_icon = match layer.kind {
                LayerKind::Done => Icon::new(IconName::CircleCheck)
                    .text_color(color(screen.palette, color_token))
                    .into_any_element(),
                LayerKind::Active => Icon::new(IconName::LoaderCircle)
                    .text_color(color(screen.palette, color_token))
                    .into_any_element(),
                LayerKind::Pending => Icon::default()
                    .path("icons/circle.svg")
                    .text_color(color(screen.palette, color_token))
                    .into_any_element(),
            };
            let progress = match layer.kind {
                LayerKind::Done => SharedString::from(gui::GUI_DESKTOP_DONE.text(locale)),
                LayerKind::Active => layer
                    .total
                    .filter(|total| *total > 0)
                    .map(|total| {
                        let current = layer.current.unwrap_or(0);
                        SharedString::from(format!(
                            "{}%",
                            ((current as f64 / total as f64) * 100.0).round() as u64
                        ))
                    })
                    .unwrap_or_else(|| SharedString::from("…")),
                LayerKind::Pending => SharedString::from("—"),
            };
            row(locale)
                .w_full()
                .items_center()
                .gap_2()
                .px(px(Space::S8.pixels()))
                .py(px(Space::S4.pixels()))
                .border_b_1()
                .border_color(tone(screen, ColorToken::BorderSubtle))
                .child(
                    div()
                        .flex_grow(1.2)
                        .flex_basis(px(0.))
                        .min_w_0()
                        .font_family("IBM Plex Mono")
                        .text_sm()
                        .text_color(tone(screen, ColorToken::TextMuted))
                        .child(isolate_technical_text(
                            &id.chars().take(12).collect::<String>(),
                        )),
                )
                .child(
                    row(locale)
                        .flex_grow(2.0)
                        .flex_basis(px(0.))
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(status_icon)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(color(screen.palette, color_token))
                                .child(layer.status.clone()),
                        ),
                )
                .child(
                    row(locale)
                        .flex_grow(1.2)
                        .flex_basis(px(0.))
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .children((layer.kind == LayerKind::Active).then(|| {
                            let fraction = layer
                                .total
                                .filter(|total| *total > 0)
                                .map(|total| layer.current.unwrap_or(0) as f32 / total as f32)
                                .unwrap_or(0.2);
                            div()
                                .flex_1()
                                .h(px(4.))
                                .rounded_full()
                                .bg(color(screen.palette, ColorToken::BgCode))
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .bg(color(screen.palette, ColorToken::AccentDefault))
                                        .w(relative(fraction.clamp(0., 1.))),
                                )
                        }))
                        .child(
                            div()
                                .text_sm()
                                .text_color(color(
                                    screen.palette,
                                    if layer.kind == LayerKind::Done {
                                        ColorToken::StatusRunning
                                    } else {
                                        ColorToken::TextMuted
                                    },
                                ))
                                .child(progress),
                        ),
                )
        }));
    div()
        .flex()
        .flex_col()
        .rounded(px(Radius::R8.pixels()))
        .border_1()
        .border_color(tone(screen, ColorToken::BorderSubtle))
        .overflow_hidden()
        .child(header)
        .child(rows.max_h(px(180.)).overflow_y_scrollbar())
}

fn header_cell(screen: &ScreenContext<'_>, key: marsa_ui::TextKey, flex: f32) -> Div {
    div()
        .flex_grow(flex)
        .flex_basis(px(0.))
        .min_w_0()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(tone(screen, ColorToken::TextMuted))
        .child(message(screen.locale, key))
}

fn output_panel(screen: &ScreenContext<'_>, view: &Entity<ResourcesView>, pull: &PullState) -> Div {
    let locale = screen.locale;
    let toggle = view.downgrade();
    let count = pull.logs.len();
    let header = row(locale).w_full().items_center().justify_between().child(
        Button::new("toggle-pull-output")
            .ghost()
            .compact()
            .label(SharedString::from(format!(
                ">_ {} ({count} {})",
                gui::GUI_DESKTOP_OUTPUT.text(locale),
                gui::GUI_DESKTOP_LINES.text(locale)
            )))
            .on_click(move |_, _, cx| {
                let _ = toggle.update(cx, |view, cx| view.toggle_pull_output(cx));
            }),
    );
    let mut panel = div()
        .flex()
        .flex_col()
        .gap(px(Space::S8.pixels()))
        .child(header);
    if pull.output_open {
        panel = panel.child(
            div()
                .w_full()
                .max_h(px(140.))
                .overflow_y_scrollbar()
                .rounded(px(Radius::R8.pixels()))
                .border_1()
                .border_color(tone(screen, ColorToken::BorderSubtle))
                .bg(tone(screen, ColorToken::BgCode))
                .p(px(Space::S8.pixels()))
                .flex()
                .flex_col()
                .gap(px(Space::S4.pixels()))
                .children(pull.logs.iter().map(|line| {
                    let badge_color = match line.kind {
                        LogKind::Pull => ColorToken::AccentDefault,
                        LogKind::Layer => ColorToken::StatusRunning,
                    };
                    let badge = match line.kind {
                        LogKind::Pull => "pull",
                        LogKind::Layer => "layer",
                    };
                    row(locale)
                        .items_start()
                        .gap_2()
                        .child(
                            div()
                                .rounded(px(Radius::Full.pixels()))
                                .px(px(Space::S4.pixels()))
                                .py(px(2.))
                                .bg(color(screen.palette, badge_color).opacity(0.2))
                                .text_xs()
                                .font_family("IBM Plex Mono")
                                .text_color(color(screen.palette, badge_color))
                                .child(badge),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .font_family("IBM Plex Mono")
                                .text_xs()
                                .text_color(tone(screen, ColorToken::TextPrimary))
                                .child(isolate_technical_text(&line.text)),
                        )
                })),
        );
    }
    panel
}

fn configure_footer(
    screen: &ScreenContext<'_>,
    view: &Entity<ResourcesView>,
    pull: &PullState,
    cx: &App,
) -> Div {
    let locale = screen.locale;
    let cancel = view.downgrade();
    let submit = view.downgrade();
    let service = screen.services.cloned();
    let name = pull.input.read(cx).value();
    let authority = pull
        .registries
        .get(pull.registry_index)
        .map(|choice| choice.authority.as_str())
        .unwrap_or("docker.io");
    let reference = compose_reference(authority, name.trim());
    let ready = ImageReference::parse(reference).is_ok();
    dialog_actions(locale, screen.palette)
        .child(
            Button::new("cancel-pull-configure")
                .outline()
                .h(px(Space::S36.pixels()))
                .rounded(px(Radius::R8.pixels()))
                .label(message(locale, gui::GUI_DESKTOP_CANCEL))
                .on_click(move |_, window, cx| {
                    let _ = cancel.update(cx, |view, cx| view.close_pull(window, cx));
                }),
        )
        .child(
            Button::new("start-image-pull")
                .primary()
                .h(px(Space::S36.pixels()))
                .rounded(px(Radius::R8.pixels()))
                .icon(Icon::default().path("icons/download.svg"))
                .label(message(locale, gui::GUI_DESKTOP_PULL))
                .disabled(!ready || service.is_none())
                .on_click(move |_, _, cx| {
                    let Some(service) = &service else {
                        return;
                    };
                    let _ = submit.update(cx, |view, cx| {
                        view.start_pull(Arc::clone(service), cx);
                    });
                }),
        )
}

fn pulling_footer(screen: &ScreenContext<'_>, view: &Entity<ResourcesView>, active: bool) -> Div {
    let locale = screen.locale;
    let cancel = view.downgrade();
    dialog_actions(locale, screen.palette).child(
        Button::new("cancel-image-pull")
            .outline()
            .h(px(Space::S36.pixels()))
            .rounded(px(Radius::R8.pixels()))
            .label(message(locale, gui::GUI_DESKTOP_CANCEL))
            .disabled(!active)
            .on_click(move |_, window, cx| {
                let _ = cancel.update(cx, |view, cx| view.cancel_pull(window, cx));
            }),
    )
}

fn complete_footer(screen: &ScreenContext<'_>, view: &Entity<ResourcesView>) -> Div {
    let locale = screen.locale;
    let remove = view.downgrade();
    let keep = view.downgrade();
    let service = screen.services.cloned();
    dialog_actions(locale, screen.palette)
        .child(
            Button::new("remove-pulled-image")
                .danger()
                .h(px(Space::S36.pixels()))
                .rounded(px(Radius::R8.pixels()))
                .icon(Icon::default().path("icons/trash.svg"))
                .label(message(locale, gui::GUI_DESKTOP_REMOVE_IMAGE))
                .disabled(service.is_none())
                .on_click(move |_, window, cx| {
                    let Some(service) = &service else {
                        return;
                    };
                    let _ = remove.update(cx, |view, cx| {
                        view.remove_pulled_image(Arc::clone(service), window, cx);
                    });
                }),
        )
        .child(
            Button::new("keep-pulled-image")
                .primary()
                .h(px(Space::S36.pixels()))
                .rounded(px(Radius::R8.pixels()))
                .icon(IconName::CircleCheck)
                .label(message(locale, gui::GUI_DESKTOP_KEEP_IMAGE))
                .on_click(move |_, window, cx| {
                    let _ = keep.update(cx, |view, cx| view.keep_pulled_image(window, cx));
                }),
        )
}

fn layer_counts(pull: &PullState) -> (usize, usize) {
    let total = pull.layers.len();
    let done = pull
        .layers
        .values()
        .filter(|layer| layer.kind == LayerKind::Done)
        .count();
    (done, total)
}

fn byte_totals(pull: &PullState) -> (u64, u64) {
    let mut downloaded = 0u64;
    let mut expected = 0u64;
    for layer in pull.layers.values() {
        if let Some(total) = layer.total {
            expected = expected.saturating_add(total);
            downloaded = downloaded.saturating_add(layer.current.unwrap_or(0).min(total));
        } else if layer.kind == LayerKind::Done {
            // Prefer progress totals; fall back to treating complete layers as counted.
        }
    }
    (downloaded, expected)
}

fn format_bytes(locale: Locale, bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!(
            "{:.1} {}",
            bytes as f64 / 1_000_000_000.,
            gui::GUI_DESKTOP_GB.text(locale)
        )
    } else if bytes >= 1_000_000 {
        format!(
            "{:.1} {}",
            bytes as f64 / 1_000_000.,
            gui::GUI_DESKTOP_MB.text(locale)
        )
    } else if bytes >= 1_000 {
        format!(
            "{:.1} {}",
            bytes as f64 / 1_000.,
            gui::GUI_DESKTOP_KB.text(locale)
        )
    } else {
        format!("{bytes} {}", gui::GUI_DESKTOP_B.text(locale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_reference_defaults_latest_on_hub() {
        assert_eq!(compose_reference("docker.io", "nginx"), "nginx:latest");
        assert_eq!(
            compose_reference("docker.io", "library/nginx:1.25"),
            "library/nginx:1.25"
        );
        assert_eq!(
            compose_reference("ghcr.io", "owner/app"),
            "ghcr.io/owner/app:latest"
        );
        assert_eq!(
            compose_reference("ghcr.io", "ghcr.io/owner/app:v1"),
            "ghcr.io/owner/app:v1"
        );
        assert_eq!(compose_reference("docker.io", "  "), "");
    }

    #[test]
    fn layer_kind_classifies_docker_status() {
        assert_eq!(layer_kind("Download complete", None), LayerKind::Done);
        assert_eq!(layer_kind("Pull complete", None), LayerKind::Done);
        assert_eq!(
            layer_kind(
                "Downloading",
                Some(&marsa_client::docker::PullCounters {
                    current: Some(1),
                    total: Some(2),
                })
            ),
            LayerKind::Active
        );
        assert_eq!(layer_kind("Pulling fs layer", None), LayerKind::Pending);
    }
}
