//! Native workspace, window commands and screen routing.

mod navigation;
mod palette;
mod sidebar;
pub(crate) mod status;

use marsa_ui::gui_keys as gui;
use std::{collections::VecDeque, sync::Arc};

use marsa_client::api::EngineState;
use marsa_ui::{
    ColorToken, Locale, Palette,
    gpui::{
        App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
        ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Task, Window,
        WindowAppearance, actions, div,
    },
    gpui_component::{
        button::{Button, ButtonVariants},
        input::InputState,
    },
    preferences::{Language, Theme},
    tokens::typography::Role,
};

use crate::{
    screens::{
        common::{ScreenContext, align, color, row},
        containers::ContainerView,
        resources::{self, ResourceKind, ResourcesView},
        system::SystemView,
    },
    services::{EngineAction, Services},
    store::AppState,
};
pub(crate) use navigation::Route;

actions!(marsa, [OpenCommandPalette, CloseCommandPalette]);

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", OpenCommandPalette, Some("MarsaDesktop")),
        KeyBinding::new("escape", CloseCommandPalette, Some("MarsaDesktop")),
    ]);
}

pub(crate) struct Desktop {
    route: Route,
    focus: FocusHandle,
    collapsed: bool,
    locale: Locale,
    system_locale: Locale,
    palette: Palette,
    container_locale: Locale,
    appearance_preference: Theme,
    state: AppState,
    services: Option<Arc<Services>>,
    history: VecDeque<EngineState>,
    containers: Entity<ContainerView>,
    resources: Entity<ResourcesView>,
    system: Entity<SystemView>,
    query: Entity<InputState>,
    palette_open: bool,
    _subscription: Subscription,
    _search_subscription: Subscription,
    _store: Option<Task<()>>,
}

impl Desktop {
    pub(crate) fn new(window: &mut Window, locale: Locale, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let initial_focus = focus.clone();
        window.on_next_frame(move |window, cx| initial_focus.focus(window, cx));
        let services = Services::start().ok().map(Arc::new);
        let palette = palette_for(window, Theme::System);
        let containers = cx.new(|cx| ContainerView::new(services.clone(), locale, palette, cx));
        let resources = cx.new(|cx| ResourcesView::new(window, cx));
        let system = cx.new(|cx| SystemView::new(services.clone(), window, cx));
        let query = cx.new(|cx| InputState::new(window, cx));
        let _subscription = cx.observe(&system, |this, system, cx| {
            let preferences = system.read(cx).preferences();
            if let Some((language, theme)) = preferences {
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
        let mut state = AppState::default();
        let store = services.as_ref().map(|services| {
            let mut selection = services.store().select(|state| state.clone());
            state = selection.current().clone();
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
        Self {
            route: Route::Overview,
            focus,
            collapsed: false,
            locale,
            system_locale: locale,
            palette,
            container_locale: locale,
            appearance_preference: Theme::System,
            state,
            services,
            history: VecDeque::new(),
            containers,
            resources,
            system,
            query,
            palette_open: false,
            _subscription,
            _search_subscription,
            _store: store,
        }
    }

    fn engine(&self) -> Option<EngineState> {
        self.state.engine().map(|engine| engine.state())
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
        self.system.update(cx, |view, cx| view.enter(route, cx));
        cx.notify();
    }

    fn request_engine(
        &mut self,
        action: EngineAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette_open = false;
        self.system
            .update(cx, |view, cx| view.engine(action, self.locale, window, cx));
        cx.notify();
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette_for(window, self.appearance_preference);
        if palette != self.palette || self.container_locale != self.locale {
            self.palette = palette;
            self.container_locale = self.locale;
            self.containers
                .update(cx, |view, cx| view.set_appearance(self.locale, palette, cx));
        }
        let screen = ScreenContext {
            locale: self.locale,
            palette,
            state: &self.state,
            services: self.services.as_ref(),
        };
        let route = self.route;
        let sidebar = sidebar::render(self, cx);
        let header = row(self.locale)
            .justify_between()
            .gap_3()
            .child(
                align(div().flex_1(), self.locale)
                    .child(
                        type_role(Role::Title, self.locale)
                            .apply(div())
                            .child(route.name(self.locale)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(color(palette, ColorToken::TextMuted))
                            .child(route.description(self.locale)),
                    ),
            )
            .child(
                Button::new("desktop-search")
                    .outline()
                    .label(gui::GUI_DESKTOP_SEARCH_K.text(self.locale))
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
            );
        let view = match route {
            Route::Containers => self.containers.clone().into_any_element(),
            Route::Images | Route::Volumes | Route::Networks | Route::Compose => {
                let kind = match route {
                    Route::Images => ResourceKind::Images,
                    Route::Volumes => ResourceKind::Volumes,
                    Route::Networks => ResourceKind::Networks,
                    _ => ResourceKind::Compose,
                };
                resources::render(&screen, kind, &self.resources, cx).into_any_element()
            }
            _ => self.system.update(cx, |system, cx| {
                system
                    .render_route(route, &screen, &self.history, cx)
                    .into_any_element()
            }),
        };
        let notice = self.system.update(cx, |view, cx| view.banner(&screen, cx));
        let content = div()
            .id(("screen", route as usize))
            .flex_1()
            .min_h_0()
            .child(view);
        let content = if route == Route::Containers {
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
            .child(header)
            .children(notice)
            .child(content);
        type_role(Role::Body, self.locale)
            .apply(div().id("marsa-desktop"))
            .track_focus(&self.focus)
            .key_context("MarsaDesktop")
            .tab_group()
            .on_action(
                cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                    this.open_palette(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CloseCommandPalette, window, cx| {
                if this.palette_open {
                    this.palette_open = false;
                    this.focus.focus(window, cx);
                    cx.notify();
                }
            }))
            .size_full()
            .relative()
            .bg(color(palette, ColorToken::BgCanvas))
            .text_color(color(palette, ColorToken::TextPrimary))
            .child(
                row(self.locale)
                    .size_full()
                    .p_2()
                    .gap_2()
                    .child(sidebar)
                    .child(main),
            )
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
