//! Native workspace, window commands and screen routing.

mod background;
mod navigation;
mod onboarding;
mod palette;
mod session;
mod sidebar;
pub(crate) mod status;
pub(crate) mod toast;

use marsa_ui::gui_keys as gui;
use std::{collections::VecDeque, sync::Arc};

use marsa_client::api::EngineState;
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        self, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
        KeyBinding, ParentElement, Pixels, Render, ScrollHandle, StatefulInteractiveElement,
        Styled, Subscription, Task, WeakFocusHandle, Window, WindowAppearance, actions, div, px,
        svg,
    },
    gpui_component::{
        IconName,
        button::{Button, ButtonVariants},
        input::InputState,
    },
    preferences::{AccentColor, Language, Theme},
    tokens::typography::Role,
};

use crate::{
    screens::{
        common::{ScreenContext, align, color, row},
        containers::ContainerView,
        external_inventory::{ContextFilter, Workspaces},
        resources::{self, ResourceKind, ResourcesView},
        system::{OpenRoute, SelectContext, SystemView},
    },
    services::{EngineAction, Services},
    store::AppState,
};
pub(crate) use navigation::Route;

actions!(
    marsa,
    [
        OpenCommandPalette,
        CloseCommandPalette,
        StartEngineFromScreen,
        NavigateToContainers,
        NavigateToDisk,
        NavigateToSettings,
    ]
);

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", OpenCommandPalette, Some("MarsaDesktop")),
        KeyBinding::new("escape", CloseCommandPalette, Some("MarsaDesktop")),
        KeyBinding::new("cmd-1", NavigateToContainers, Some("MarsaDesktop")),
        KeyBinding::new("cmd-6", NavigateToDisk, Some("MarsaDesktop")),
        KeyBinding::new("cmd-,", NavigateToSettings, Some("MarsaDesktop")),
    ]);
}

pub(crate) struct Desktop {
    route: Route,
    focus: FocusHandle,
    collapsed: bool,
    locale: Locale,
    system_locale: Locale,
    palette: Palette,
    themed_palette: Option<(Palette, Locale)>,
    container_locale: Locale,
    controls_layout: Option<(Locale, Pixels, bool)>,
    appearance_preference: Theme,
    accent: AccentColor,
    state: AppState,
    external_inventory: Workspaces,
    context_filter: Entity<ContextFilter>,
    _context_subscription: Subscription,
    _context_picker_subscription: Subscription,
    services: Option<Arc<Services>>,
    history: VecDeque<EngineState>,
    containers: Entity<ContainerView>,
    resources: Entity<ResourcesView>,
    system: Entity<SystemView>,
    query: Entity<InputState>,
    palette_open: bool,
    palette_focus: FocusHandle,
    palette_previous_focus: Option<WeakFocusHandle>,
    palette_index: usize,
    palette_scroll: ScrollHandle,
    _subscription: Subscription,
    _search_subscription: Subscription,
    _resource_subscription: marsa_ui::gpui::Subscription,
    _route_subscription: Subscription,
    _store: Option<Task<()>>,
    _setup_probe: Option<Task<()>>,
    onboarding: Option<onboarding::Setup>,
    setup_task: Option<Task<()>>,
}

impl Desktop {
    pub(crate) fn new(window: &mut Window, locale: Locale, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let initial_focus = focus.clone();
        window.on_next_frame(move |window, cx| initial_focus.focus(window, cx));
        let services = session::attach(window, locale, cx);
        let palette = palette_for(window, Theme::System);
        let context_filter = cx.new(|_| ContextFilter::default());
        let _context_subscription = cx.observe(&context_filter, |_, _, cx| cx.notify());
        let containers = cx.new(|cx| {
            ContainerView::new(
                services.clone(),
                locale,
                palette,
                context_filter.clone(),
                cx,
            )
        });
        let resources = cx.new(|cx| ResourcesView::new(window, cx));
        let system = cx.new(|cx| SystemView::new(services.clone(), window, cx));
        let _context_picker_subscription =
            cx.subscribe(&system, |this, _, SelectContext(selection), cx| {
                this.context_filter.update(cx, |filter, cx| {
                    *filter = selection.clone();
                    cx.notify();
                });
            });
        let query = cx.new(|cx| InputState::new(window, cx));
        let _resource_subscription = cx.subscribe(
            &resources,
            |this, _, crate::screens::resources::ExternalResourcesChanged, cx| {
                this.system
                    .update(cx, |view, cx| view.refresh_external_inventory(cx));
            },
        );
        let _route_subscription = cx.subscribe(&system, |this, _, OpenRoute(route), cx| {
            this.navigate(*route, cx);
        });
        let _subscription = cx.observe(&system, |this, system, cx| {
            let preferences = system.read(cx).preferences();
            this.accent = system.read(cx).accent();
            let inventory = system.read(cx).external_inventory();
            if !Arc::ptr_eq(&this.external_inventory, inventory) {
                this.external_inventory = inventory.clone();
                this.containers.update(cx, |view, cx| {
                    view.set_external_inventory(this.external_inventory.clone(), cx);
                });
            }

            if this.onboarding.is_none()
                && let Some((language, theme)) = preferences
            {
                if this.appearance_preference != theme {
                    this.appearance_preference = theme;
                    use marsa_ui::gpui_component::{Theme as ComponentTheme, ThemeMode};
                    match theme {
                        Theme::System => ComponentTheme::sync_system_appearance(None, cx),
                        Theme::Light => ComponentTheme::change(ThemeMode::Light, None, cx),
                        Theme::Dark => ComponentTheme::change(ThemeMode::Dark, None, cx),
                    }
                }
                let locale = match language {
                    Language::System => this.system_locale,
                    Language::English => Locale::EnglishUs,
                    Language::Arabic => Locale::Arabic,
                };
                if locale != this.locale {
                    this.locale = locale;
                    marsa_ui::direction::set_locale(locale, cx);
                }
            }
            cx.notify();
        });
        system.update(cx, |view, cx| view.refresh_external_inventory(cx));
        let mut state = AppState::default();
        let store = services.as_ref().map(|services| {
            let mut selection = services.store().select(|state| state.clone());
            state = selection.current().clone();
            system.update(cx, |view, cx| view.observe(state.daemon(), cx));
            cx.spawn(async move |this, cx| {
                while let Some(state) = selection.changed().await {
                    if this
                        .update(cx, |this, cx| {
                            let before = this.engine();
                            this.state = state.clone();
                            if let Some(current) =
                                this.engine().filter(|current| Some(*current) != before)
                            {
                                if this.history.len() == 64 {
                                    this.history.pop_front();
                                }
                                this.history.push_back(current);
                            }
                            let observed = this.state.daemon().cloned();
                            this.system
                                .update(cx, |view, cx| view.observe(observed.as_ref(), cx));
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let _search_subscription = palette::observe_query(&query, window, cx);
        let _setup_probe = services
            .as_deref()
            .map(|services| onboarding::probe(services, cx));
        Self {
            route: Route::Overview,
            focus,
            collapsed: false,
            locale,
            system_locale: locale,
            palette,
            themed_palette: None,
            container_locale: locale,
            controls_layout: None,
            appearance_preference: Theme::System,
            accent: AccentColor::MarsaBlue,
            state,
            external_inventory: Arc::from([]),
            context_filter,
            _context_subscription,
            _context_picker_subscription,
            services,
            history: VecDeque::new(),
            containers,
            resources,
            system,
            query,
            palette_open: false,
            palette_focus: cx.focus_handle(),
            palette_previous_focus: None,
            palette_index: 0,
            palette_scroll: ScrollHandle::new(),
            _subscription,
            _search_subscription,
            _resource_subscription,
            _route_subscription,
            _store: store,
            _setup_probe,
            onboarding: None,
            setup_task: None,
        }
    }

    fn engine(&self) -> Option<EngineState> {
        self.state.engine().map(|engine| engine.state())
    }

    fn context_label(&self, cx: &App) -> gpui::SharedString {
        match self.context_filter.read(cx) {
            ContextFilter::All | ContextFilter::Marsa => "marsa".into(),
            filter => filter.display(self.locale),
        }
    }

    fn context_engine(&self, cx: &App) -> Option<EngineState> {
        let ContextFilter::External { endpoint, .. } = self.context_filter.read(cx) else {
            return self.engine();
        };
        Some(
            if self
                .external_inventory
                .iter()
                .any(|workspace| &workspace.endpoint == endpoint && workspace.reachable)
            {
                EngineState::Ready
            } else {
                EngineState::Failed
            },
        )
    }

    fn open_contexts(&mut self, cx: &mut Context<Self>) {
        self.system
            .update(cx, |system, cx| system.select_docker_settings(cx));
        self.navigate(Route::Settings, cx);
    }

    fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route == Route::Containers && route != Route::Containers {
            self.containers.update(cx, |view, cx| view.leave(cx));
        }
        if matches!(
            self.route,
            Route::Images | Route::Volumes | Route::Networks | Route::Compose
        ) && self.route != route
        {
            self.resources.update(cx, |view, _| view.leave());
        }
        self.route = route;
        self.palette_open = false;
        self.palette_previous_focus = None;
        self.system.update(cx, |view, cx| {
            view.enter(route, cx);
            if matches!(
                route,
                Route::Containers
                    | Route::Images
                    | Route::Volumes
                    | Route::Networks
                    | Route::Compose
            ) {
                view.refresh_external_inventory(cx);
            }
        });
        cx.notify();
    }

    fn request_engine(
        &mut self,
        action: EngineAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette_open = false;
        self.palette_previous_focus = None;
        self.system
            .update(cx, |view, cx| view.engine(action, self.locale, window, cx));
        cx.notify();
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controls_layout = (self.locale, window.viewport_size().width, self.collapsed);
        if self.controls_layout != Some(controls_layout) {
            self.controls_layout = Some(controls_layout);
            marsa_ui::direction::position_window_controls(window, self.locale, self.collapsed);
        }
        let palette = palette_for(window, self.appearance_preference).with_accent(self.accent);
        if self.themed_palette != Some((palette, self.locale)) {
            self.themed_palette = Some((palette, self.locale));
            self.resources
                .update(cx, |view, cx| view.set_locale(self.locale, window, cx));
            palette.apply_to_components(self.locale, cx);
        }
        if palette != self.palette || self.container_locale != self.locale {
            self.palette = palette;
            self.container_locale = self.locale;
            self.containers.update(cx, |view, cx| {
                view.set_appearance(self.locale, palette, window, cx)
            });
        }
        let screen = ScreenContext {
            locale: self.locale,
            palette,
            state: &self.state,
            services: self.services.as_ref(),
        };
        let route = self.route;
        let failed = (route == Route::Machine
            || route == Route::Containers && self.external_inventory.is_empty())
            && self.engine() == Some(EngineState::Failed);
        let sidebar = sidebar::render(self, cx);
        let header = row(self.locale)
            .justify_between()
            .gap_3()
            .child(
                align(div().flex_1(), self.locale)
                    .child(
                        type_role(Role::Title, self.locale)
                            .apply(div())
                            .child(if failed {
                                gui::GUI_SYSTEM_ENGINE_FAILED_TITLE.text(self.locale)
                            } else {
                                route.name(self.locale)
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(color(palette, ColorToken::TextMuted))
                            .child(if failed {
                                gui::GUI_SYSTEM_ENGINE_FAILED_SUBTITLE.text(self.locale)
                            } else {
                                route.description(self.locale)
                            }),
                    ),
            )
            .child(
                Button::new("desktop-search")
                    .ghost()
                    .size(px(36.))
                    .p_0()
                    .bg(color(palette, ColorToken::GlassControl))
                    .accessibility_label(gui::GUI_DESKTOP_SEARCH_COMMANDS.text(self.locale))
                    .tooltip(gui::GUI_DESKTOP_SEARCH_K.text(self.locale))
                    .child(IconName::Search)
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
            )
            .children(
                (route == Route::Overview
                    && matches!(
                        self.engine(),
                        Some(EngineState::Stopped | EngineState::Failed)
                    ))
                .then(|| {
                    Button::new("start-engine-from-overview")
                        .primary()
                        .label(gui::GUI_DESKTOP_START_MACHINE.text(self.locale))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.request_engine(EngineAction::Start, window, cx);
                        }))
                }),
            )
            .children((route == Route::Overview).then(|| {
                Button::new("overview-open-containers")
                    .primary()
                    .size(px(36.))
                    .p_0()
                    .bg(color(palette, ColorToken::AccentDefault))
                    .accessibility_label(gui::GUI_DESKTOP_OPEN_CONTAINERS.text(self.locale))
                    .tooltip(gui::GUI_DESKTOP_OPEN_CONTAINERS.text(self.locale))
                    .child(
                        svg()
                            .path(marsa_ui::direction::launch_arrow(self.locale))
                            .size(px(18.))
                            .text_color(color(palette, ColorToken::TextOnAccent)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.navigate(Route::Containers, cx)))
            }));
        let view = if failed {
            self.system.update(cx, |view, cx| {
                view.failed_engine(&screen, cx).into_any_element()
            })
        } else {
            match route {
                Route::Containers => self.containers.clone().into_any_element(),
                Route::Images | Route::Volumes | Route::Networks | Route::Compose => {
                    let kind = match route {
                        Route::Images => ResourceKind::Images,
                        Route::Volumes => ResourceKind::Volumes,
                        Route::Networks => ResourceKind::Networks,
                        _ => ResourceKind::Compose,
                    };
                    resources::render(
                        &screen,
                        kind,
                        &self.resources,
                        &self.external_inventory,
                        &self.context_filter,
                        cx,
                    )
                    .into_any_element()
                }
                _ => self.system.update(cx, |system, cx| {
                    system
                        .render_route(route, &screen, &self.history, cx)
                        .into_any_element()
                }),
            }
        };
        let toast = self
            .resources
            .update(cx, |view, cx| view.toast(&screen, cx))
            .or_else(|| self.containers.update(cx, |view, cx| view.toast(cx)))
            .or_else(|| self.system.update(cx, |view, cx| view.toast(&screen, cx)));
        let content = div()
            .id(("screen", route as usize))
            .flex_1()
            .min_h_0()
            .child(view);
        let content = if route == Route::Containers && !failed {
            content.overflow_hidden()
        } else {
            content.overflow_y_scroll()
        };
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .children((route != Route::Containers || failed).then_some(header))
            .child(content);
        type_role(Role::Body, self.locale)
            .apply(div().id("marsa-desktop"))
            .track_focus(&self.focus)
            .key_context("MarsaDesktop")
            .tab_group()
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                if this.onboarding.is_none() {
                    this.open_palette(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseCommandPalette, window, cx| {
                if this.onboarding.is_some() {
                    this.dismiss_setup(cx);
                } else {
                    this.close_palette(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &StartEngineFromScreen, window, cx| {
                this.request_engine(EngineAction::Start, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateToContainers, window, cx| {
                this.close_palette(window, cx);
                this.navigate(Route::Containers, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateToDisk, window, cx| {
                this.close_palette(window, cx);
                this.navigate(Route::Disk, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateToSettings, window, cx| {
                this.close_palette(window, cx);
                this.navigate(Route::Settings, cx);
            }))
            .size_full()
            .relative()
            .bg(color(palette, ColorToken::BgCanvas))
            .child(background::render(palette, self.accent))
            .text_color(color(palette, ColorToken::TextPrimary))
            .child(if self.onboarding.is_some() {
                self.setup_view(cx).into_any_element()
            } else {
                row(self.locale)
                    .size_full()
                    .p_2()
                    .child(sidebar)
                    .child(main)
                    .into_any_element()
            })
            .children(self.onboarding.is_none().then_some(toast).flatten())
            .children(self.palette_open.then(|| self.command_palette(cx)))
    }
}

fn palette_for(window: &Window, preference: Theme) -> Palette {
    use marsa_ui::tokens::Appearance;
    let appearance = match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
        _ => Appearance::Light,
    };
    Palette::resolve(preference, appearance)
}

fn type_role(role: Role, locale: Locale) -> marsa_ui::tokens::typography::TypeStyle {
    if locale == Locale::Arabic {
        role.arabic()
    } else {
        role.latin()
    }
}
