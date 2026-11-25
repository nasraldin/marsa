//! Render two keyboard-operable tables in one native window.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    native::run();
}

#[cfg(target_os = "macos")]
mod native {
    use std::rc::Rc;

    use marsa_ui::{
        Locale, Palette,
        components::table::{Column, DataTable, TableDelegate, TableState},
        direction::{self, row_for},
        gpui::{
            App, AppContext as _, Application, Bounds, Context, Element as _, ElementId, Entity,
            Focusable as _, IntoElement, Keystroke, ParentElement as _, Render, RenderOnce as _,
            SharedString, Styled as _, Window, WindowBounds, WindowOptions, WindowingRequest, div,
            px, size,
        },
        gpui_component,
        preferences::DisplaySettings,
    };

    struct Rows(usize);

    impl TableDelegate for Rows {
        fn columns_count(&self, _: &App) -> usize {
            1
        }

        fn rows_count(&self, _: &App) -> usize {
            self.0
        }

        fn column(&self, _: usize, _: &App) -> Column {
            Column::new("name", "Resource")
        }

        fn render_td(
            &mut self,
            row: usize,
            _: usize,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) -> impl IntoElement {
            div().child(format!("sample-{row}"))
        }

        fn cell_label(&self, row: usize, _: usize, _: &App) -> Option<SharedString> {
            Some(format!("sample-{row}").into())
        }
    }

    struct SiblingTables {
        tables: [Entity<TableState<Rows>>; 2],
        locale: Locale,
    }

    impl Render for SiblingTables {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            row_for(self.locale).size_full().children(
                self.tables
                    .iter()
                    .map(|state| div().flex_1().min_w_0().child(DataTable::new(state))),
            )
        }
    }

    pub(super) fn run() {
        Application::with_platform(Rc::new(gpui_macos::MacPlatform::new(false)))
            .with_windowing(WindowingRequest::Headless)
            .run(|cx| {
                gpui_component::init(cx);
                direction::init(Locale::EnglishUs, cx);
                for (locale, palette) in [
                    (Locale::EnglishUs, Palette::Light),
                    (Locale::EnglishUs, Palette::Dark),
                    (Locale::Arabic, Palette::Light),
                    (Locale::Arabic, Palette::Dark),
                ] {
                    direction::set_locale(locale, cx);
                    palette.apply_to_components(locale, DisplaySettings::default(), cx);
                    verify(locale, cx);
                }
                std::process::exit(0);
            });
    }

    fn verify(locale: Locale, cx: &mut App) {
        let options = WindowOptions {
            focus: false,
            show: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(700.), px(380.)),
                cx,
            ))),
            ..WindowOptions::default()
        };
        let handle = cx
            .open_window(options, |window, cx| {
                cx.new(|cx| SiblingTables {
                    tables: [3, 2_000]
                        .map(|count| cx.new(|cx| TableState::new(Rows(count), window, cx))),
                    locale,
                })
            })
            .expect("open hidden window");
        cx.update_window(handle.into(), |root, window, cx| {
            let tables = root
                .downcast::<SiblingTables>()
                .unwrap()
                .read(cx)
                .tables
                .clone();
            let ids = table_ids(&tables, window, cx);
            assert_ne!(ids[0], ids[1], "sibling tables need distinct AX node IDs");
            window.draw(cx).clear(cx);
            let first = tables[0].read(cx).focus_handle(cx);
            first.focus(window, cx);
            send_down(window, cx);
            assert_eq!(tables[0].read(cx).selected_row(), Some(0));
            assert_eq!(tables[1].read(cx).selected_row(), None);

            window.focus_next(cx);
            assert!(tables[1].read(cx).focus_handle(cx).is_focused(window));
            send_down(window, cx);
            send_down(window, cx);
            assert_eq!(tables[0].read(cx).selected_row(), Some(0));
            assert_eq!(tables[1].read(cx).selected_row(), Some(1));
            assert_eq!(ids, table_ids(&tables, window, cx));
            window.remove_window();
        })
        .expect("render and operate sibling tables");
    }

    fn table_ids(
        tables: &[Entity<TableState<Rows>>; 2],
        window: &mut Window,
        cx: &mut App,
    ) -> [ElementId; 2] {
        tables.each_ref().map(|state| {
            DataTable::new(state)
                .render(window, cx)
                .into_element()
                .id()
                .expect("the DataTable accessibility root has an identity")
        })
    }

    fn send_down(window: &mut Window, cx: &mut App) {
        window.draw(cx).clear(cx);
        window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx);
    }
}
