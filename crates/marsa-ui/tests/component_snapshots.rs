//! Compare the production gallery controls in both locales and appearances.

#[cfg(target_os = "macos")]
#[allow(
    dead_code,
    unused_imports,
    reason = "the snapshot target uses the desktop's asset loader without its unit-test harness"
)]
#[path = "../src/assets.rs"]
mod assets;
#[cfg(target_os = "macos")]
#[allow(
    dead_code,
    reason = "the gallery's interactive entry point is compiled separately"
)]
#[path = "../examples/gallery/native.rs"]
mod gallery;
#[cfg(target_os = "macos")]
#[path = "support/snapshots.rs"]
mod snapshots;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    native::run();
}

#[cfg(target_os = "macos")]
mod native {
    use std::{cell::Cell, rc::Rc, time::Duration};

    use marsa_ui::{
        Locale, TextKey, direction,
        gpui::{AppContext as _, MouseMoveEvent, Pixels, PlatformInput, point, px, size},
        gpui_component::{self, Root, WindowExt as _},
        gui_keys as gui,
        preferences::DisplaySettings,
        tokens::typography::rem_size,
    };

    use super::{
        gallery::Gallery,
        snapshots::{self, SnapshotSuite, Variant},
    };

    #[derive(Clone, Copy)]
    enum State {
        Rest,
        Activate,
        Hover,
    }

    struct Case {
        name: &'static str,
        card: TextKey,
        state: State,
    }

    const CASES: &[Case] = &[
        case("button", gui::GUI_GALLERY_BUTTON),
        case("icon-button", gui::GUI_GALLERY_ICON_BUTTON),
        case("text-input", gui::GUI_GALLERY_TEXT_INPUT),
        case("search", gui::GUI_GALLERY_SEARCH),
        case("select", gui::GUI_GALLERY_SELECT),
        case("checkbox", gui::GUI_GALLERY_CHECKBOX),
        case("switch", gui::GUI_GALLERY_SWITCH),
        case("tabs", gui::GUI_GALLERY_TABS),
        case("list", gui::GUI_GALLERY_LIST),
        case("virtual-list", gui::GUI_GALLERY_VIRTUAL_LIST),
        case("table", gui::GUI_GALLERY_TABLE),
        case("virtual-table", gui::GUI_GALLERY_VIRTUAL_TABLE),
        case("tree", gui::GUI_GALLERY_TREE),
        active("dialog", gui::GUI_GALLERY_DIALOG),
        active("typed-confirmation", gui::GUI_GALLERY_CONFIRMATION),
        active("toast", gui::GUI_GALLERY_TOAST),
        active("menu", gui::GUI_GALLERY_MENU),
        Case {
            name: "tooltip",
            card: gui::GUI_GALLERY_TOOLTIP,
            state: State::Hover,
        },
        case("badge", gui::GUI_GALLERY_BADGE),
        case("status-pill", gui::GUI_GALLERY_STATUS),
        case("progress", gui::GUI_GALLERY_PROGRESS),
        case("empty-state", gui::GUI_GALLERY_EMPTY),
        case("error-state", gui::GUI_GALLERY_ERROR),
        case("monospace-block", gui::GUI_GALLERY_CODE),
        case("split-view", gui::GUI_GALLERY_SPLIT),
    ];

    const fn case(name: &'static str, card: TextKey) -> Case {
        Case {
            name,
            card,
            state: State::Rest,
        }
    }

    const fn active(name: &'static str, card: TextKey) -> Case {
        Case {
            name,
            card,
            state: State::Activate,
        }
    }

    pub(super) fn run() {
        snapshots::application().run(|cx| {
            super::assets::register_fonts(cx).expect("register production fonts");
            gpui_component::init(cx);
            direction::init(Locale::EnglishUs, cx);
            cx.set_reduce_motion(true);
            let timer = cx.background_executor().clone();
            cx.spawn(async move |cx| {
                let mut suite = SnapshotSuite::new();
                for variant in Variant::ALL {
                    let display = DisplaySettings::default();
                    cx.update(|cx| {
                        direction::set_locale(variant.locale, cx);
                        variant
                            .palette
                            .apply_to_components(variant.locale, display, cx);
                    });
                    for case in CASES {
                        let centers = Rc::new(Cell::new([px(0.); 2]));
                        let handle = cx.update(|cx| {
                            let handle =
                                snapshots::open(cx, size(px(540.), px(320.)), |window, cx| {
                                    window.set_rem_size(px(rem_size(display.ui_font_size)));
                                    let gallery = cx.new(|cx| {
                                        Gallery::new(variant.locale, variant.palette, window, cx)
                                            .for_snapshot(case.card, Rc::clone(&centers))
                                    });
                                    cx.new(|cx| Root::new(gallery, window, cx))
                                });
                            cx.update_window(handle, |_, window, cx| {
                                match case.state {
                                    State::Rest => {}
                                    State::Activate => {
                                        window.focus_next(cx);
                                        snapshots::key(window, cx, "space");
                                    }
                                    State::Hover => {
                                        window.dispatch_event(
                                            PlatformInput::MouseMove(MouseMoveEvent {
                                                position: point(px(100.), px(80.)),
                                                pressed_button: None,
                                                modifiers: Default::default(),
                                            }),
                                            cx,
                                        );
                                    }
                                }
                                window.draw(cx).clear(cx);
                            })
                            .expect("prepare gallery fixture");
                            handle
                        });
                        match case.state {
                            State::Rest => {}
                            State::Activate => timer.timer(Duration::from_millis(250)).await,
                            State::Hover => timer.timer(Duration::from_millis(800)).await,
                        }
                        let actual = cx.update(|cx| {
                            cx.update_window(handle, |_, window, cx| {
                                if matches!(case.name, "dialog" | "typed-confirmation") {
                                    assert!(window.has_active_dialog(cx), "render the open dialog");
                                }
                                let frame = snapshots::capture(window, cx);
                                window.remove_window();
                                frame
                            })
                            .expect("render gallery fixture")
                        });
                        if case.card == gui::GUI_GALLERY_SPLIT {
                            assert_split_order(variant.locale, centers.get());
                        }
                        suite.compare(case.name, variant, &actual);
                    }
                }
                std::process::exit(i32::from(!suite.finish(CASES.len())));
            })
            .detach();
        });
    }

    fn assert_split_order(locale: Locale, [start, end]: [Pixels; 2]) {
        assert_ne!(start, end, "both panes are laid out");
        assert_eq!(
            start > end,
            locale == Locale::Arabic,
            "the first pane starts on the logical edge"
        );
    }
}
