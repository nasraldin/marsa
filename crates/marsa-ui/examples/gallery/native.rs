//! Exercise the shared production components together in one native window.

#[path = "data.rs"]
mod data;

use std::process::ExitCode;

use marsa_ui::{
    ColorToken, Locale, Palette, TextKey,
    components::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        dialogs::{dialog_actions, dialog_frame, dialog_heading},
        feedback::{empty_state, error_state, monospace_block},
        indicators::{status_pill, tag_badge},
        inputs::{TypedConfirmation, resource_input, search_input},
        list::{List, ListState},
        menu::{DropdownMenu as _, PopupMenuItem},
        notification::Notification,
        progress::Progress,
        select::{Select, SelectState},
        split::SplitView,
        switch::Switch,
        tab::{Tab, TabBar},
        table::{DataTable, TableState},
        tree::TreeState,
    },
    direction::{align_to_start, position_window_controls, row_for, set_locale},
    gpui::{
        AppContext as _, ClipboardItem, Context, Div, Entity, FontWeight, Hsla,
        InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
        StatefulInteractiveElement as _, Styled as _, Window, div, px,
    },
    gpui_component::{
        Disableable as _, IconName, IndexPath, Theme, WindowExt as _,
        input::{InputEvent, InputState},
        scroll::ScrollableElement as _,
    },
    gui_keys as gui,
    preferences::DisplaySettings,
    tokens::{
        scale::{Radius, Space},
        typography::{Role, rem_size},
    },
};

const COMMAND: &str = "docker ps --filter status=running";
const PHRASE: &str = "sample-0001";

pub(super) fn run() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let locale = match arguments.next().as_deref() {
        Some("en-US") => Some(Locale::EnglishUs),
        Some("ar") => Some(Locale::Arabic),
        _ => None,
    };
    let appearance = match arguments.next().as_deref() {
        Some("light") => Some(Palette::Light),
        Some("dark") => Some(Palette::Dark),
        _ => None,
    };
    marsa_ui::run(move |window, cx, system_locale| {
        let locale = locale.unwrap_or(system_locale);
        let palette = appearance.unwrap_or_else(|| {
            if Theme::global(cx).is_dark() {
                Palette::Dark
            } else {
                Palette::Light
            }
        });
        let display = DisplaySettings::default();
        set_locale(locale, cx);
        palette.apply_to_components(locale, display, cx);
        window.set_rem_size(px(rem_size(display.ui_font_size)));
        window.set_window_title(gui::GUI_GALLERY_TITLE.text(locale));
        position_window_controls(window, locale, true);
        cx.new(|cx| Gallery::new(locale, palette, window, cx))
    })
}

struct Gallery {
    locale: Locale,
    palette: Palette,
    text: Entity<InputState>,
    search: Entity<InputState>,
    select: Entity<SelectState<Vec<&'static str>>>,
    lists: [Entity<ListState<data::Rows>>; 2],
    tables: [Entity<TableState<data::Rows>>; 2],
    tree: Entity<TreeState>,
    split: SplitView,
    scroll: ScrollHandle,
    checked: bool,
    live: bool,
    tab: usize,
    progress: f32,
    help: bool,
}

impl Gallery {
    fn new(locale: Locale, palette: Palette, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = cx.new(|cx| {
            InputState::new(window, cx).placeholder(gui::GUI_GALLERY_TYPE_NAME.text(locale))
        });
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder(gui::GUI_GALLERY_FIND.text(locale)));
        let select = cx.new(|cx| {
            SelectState::new(
                vec![
                    gui::GUI_GALLERY_SUMMARY.text(locale),
                    gui::GUI_GALLERY_DETAILS.text(locale),
                ],
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let lists = [3, 2_000].map(|count| {
            cx.new(|cx| {
                ListState::new(data::Rows::new(count, locale), window, cx).searchable(false)
            })
        });
        cx.subscribe_in(&search, window, |gallery, input, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value();
                gallery.lists[1].update(cx, |list, cx| {
                    list.delegate_mut().set_filter(query.as_ref());
                    list.set_selected_index(None, window, cx);
                    cx.notify();
                });
            }
        })
        .detach();
        let tables = [3, 2_000]
            .map(|count| cx.new(|cx| TableState::new(data::Rows::new(count, locale), window, cx)));
        let tree = cx.new(|cx| TreeState::new(cx).items(data::tree_items(locale)));
        Self {
            locale,
            palette,
            text,
            search,
            select,
            lists,
            tables,
            tree,
            split: SplitView::new(cx),
            scroll: ScrollHandle::new(),
            checked: false,
            live: true,
            tab: 0,
            progress: 40.,
            help: false,
        }
    }

    fn label(&self, key: TextKey) -> &'static str {
        key.text(self.locale)
    }

    fn card(&self, title: TextKey, content: impl IntoElement) -> Div {
        align_to_start(div(), self.locale)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(Space::S8.pixels()))
            .p(px(Space::S12.pixels()))
            .rounded(px(Radius::R12.pixels()))
            .border_1()
            .border_color(self.palette.color(ColorToken::BorderSubtle))
            .bg(Hsla::from(self.palette.color(ColorToken::BgElevated)))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.label(title)),
            )
            .child(content)
    }

    fn advance(&mut self, cx: &mut Context<Self>) {
        self.progress = if self.progress == 100. {
            0.
        } else {
            self.progress + 10.
        };
        cx.notify();
    }

    fn controls(&self, cx: &mut Context<Self>) -> Div {
        let label = |key| self.label(key);
        let locale = self.locale;
        let palette = self.palette;
        column()
            .child(
                self.card(
                    gui::GUI_GALLERY_BUTTON,
                    Button::new("advance")
                        .primary()
                        .label(label(gui::GUI_GALLERY_ADVANCE))
                        .on_click(cx.listener(|this, _, _, cx| this.advance(cx))),
                ),
            )
            .child(
                self.card(
                    gui::GUI_GALLERY_ICON_BUTTON,
                    row_for(locale).child(
                        Button::new("advance-icon")
                            .ghost()
                            .icon(IconName::Plus)
                            .accessibility_label(label(gui::GUI_GALLERY_ADVANCE))
                            .on_click(cx.listener(|this, _, _, cx| this.advance(cx))),
                    ),
                ),
            )
            .child(self.card(
                gui::GUI_GALLERY_TEXT_INPUT,
                resource_input(&self.text).aria_label(label(gui::GUI_GALLERY_TYPE_NAME)),
            ))
            .child(self.card(
                gui::GUI_GALLERY_SEARCH,
                search_input(&self.search, label(gui::GUI_GALLERY_FIND)),
            ))
            .child(self.card(
                gui::GUI_GALLERY_SELECT,
                Select::new(&self.select).accessibility_label(label(gui::GUI_GALLERY_SELECT)),
            ))
            .child(
                self.card(
                    gui::GUI_GALLERY_CHECKBOX,
                    Checkbox::new("include")
                        .checked(self.checked)
                        .label(label(gui::GUI_GALLERY_INCLUDE))
                        .on_change(cx.listener(|this, value, _, cx| {
                            this.checked = *value;
                            cx.notify();
                        })),
                ),
            )
            .child(
                self.card(
                    gui::GUI_GALLERY_SWITCH,
                    Switch::new("live")
                        .checked(self.live)
                        .label(label(gui::GUI_GALLERY_LIVE))
                        .on_change(cx.listener(|this, value, _, cx| {
                            this.live = *value;
                            cx.notify();
                        })),
                ),
            )
            .child(
                self.card(
                    gui::GUI_GALLERY_TABS,
                    TabBar::new("gallery-tabs")
                        .selected_index(self.tab)
                        .child(Tab::new().label(label(gui::GUI_GALLERY_SUMMARY)))
                        .child(Tab::new().label(label(gui::GUI_GALLERY_DETAILS)))
                        .on_click(cx.listener(|this, index, _, cx| {
                            this.tab = *index;
                            cx.notify();
                        })),
                ),
            )
            .child(self.card(
                gui::GUI_GALLERY_SPLIT,
                div().h(px(Space::S80.pixels())).child(self.split.render(
                    "gallery-split",
                    locale,
                    palette,
                    label(gui::GUI_GALLERY_RESIZE),
                    [gui::GUI_GALLERY_SUMMARY, gui::GUI_GALLERY_DETAILS].map(|key| {
                        align_to_start(div().w_full(), locale)
                            .p(px(Space::S8.pixels()))
                            .child(label(key))
                            .into_any_element()
                    }),
                )),
            ))
    }

    fn data(&self) -> Div {
        let list = |index| {
            div()
                .h(px(Space::S80.pixels()))
                .child(List::new(&self.lists[index]))
        };
        let table = |index, title| {
            div()
                .h(px(Space::S80.pixels() * 2.))
                .child(DataTable::new(&self.tables[index]).accessibility_label(self.label(title)))
        };
        column()
            .child(self.card(gui::GUI_GALLERY_LIST, list(0)))
            .child(self.card(gui::GUI_GALLERY_VIRTUAL_LIST, list(1)))
            .child(self.card(gui::GUI_GALLERY_TABLE, table(0, gui::GUI_GALLERY_TABLE)))
            .child(self.card(
                gui::GUI_GALLERY_VIRTUAL_TABLE,
                table(1, gui::GUI_GALLERY_VIRTUAL_TABLE),
            ))
            .child(
                self.card(
                    gui::GUI_GALLERY_TREE,
                    div()
                        .h(px(Space::S80.pixels() * 2.))
                        .child(data::example_tree(&self.tree, self.locale)),
                ),
            )
    }

    fn feedback(&self, cx: &mut Context<Self>) -> Div {
        let locale = self.locale;
        let palette = self.palette;
        let view = cx.weak_entity();
        let label = |key| self.label(key);
        let menu = Button::new("gallery-menu")
            .label(label(gui::GUI_GALLERY_MENU))
            .dropdown_menu(move |menu, _, _| {
                [gui::GUI_GALLERY_SUMMARY, gui::GUI_GALLERY_DETAILS]
                    .into_iter()
                    .enumerate()
                    .fold(menu, |menu, (index, key)| {
                        let view = view.clone();
                        menu.item(
                            PopupMenuItem::new(key.text(locale)).on_click(move |_, _, cx| {
                                let _ = view.update(cx, |gallery, cx| {
                                    gallery.tab = index;
                                    cx.notify();
                                });
                            }),
                        )
                    })
            });
        column()
            .child(
                self.card(
                    gui::GUI_GALLERY_DIALOG,
                    Button::new("dialog")
                        .label(label(gui::GUI_GALLERY_OPEN_DIALOG))
                        .on_click(move |_, window, cx| {
                            open_dialog(false, locale, palette, window, cx)
                        }),
                ),
            )
            .child(
                self.card(
                    gui::GUI_GALLERY_CONFIRMATION,
                    Button::new("confirmation")
                        .label(label(gui::GUI_GALLERY_OPEN_CONFIRMATION))
                        .on_click(move |_, window, cx| {
                            open_dialog(true, locale, palette, window, cx)
                        }),
                ),
            )
            .child(
                self.card(
                    gui::GUI_GALLERY_TOAST,
                    Button::new("toast")
                        .label(label(gui::GUI_GALLERY_SHOW_TOAST))
                        .on_click(move |_, window, cx| {
                            window.push_notification(
                                Notification::new()
                                    .message(gui::GUI_GALLERY_SAVED.text(locale))
                                    .autohide(false),
                                cx,
                            );
                        }),
                ),
            )
            .child(self.card(gui::GUI_GALLERY_MENU, menu))
            .child(
                self.card(
                    gui::GUI_GALLERY_TOOLTIP,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::S8.pixels()))
                        .child(
                            Button::new("help")
                                .label(label(gui::GUI_GALLERY_HELP))
                                .tooltip(label(gui::GUI_GALLERY_HELP_COPY))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.help = !this.help;
                                    cx.notify();
                                })),
                        )
                        .children(
                            self.help
                                .then(|| div().child(label(gui::GUI_GALLERY_HELP_COPY))),
                        ),
                ),
            )
            .child(self.card(
                gui::GUI_GALLERY_BADGE,
                row_for(locale).child(tag_badge(
                    locale,
                    palette,
                    "sample:latest".into(),
                    ColorToken::TextPrimary,
                    ColorToken::BgSubtle,
                )),
            ))
            .child(self.card(
                gui::GUI_GALLERY_STATUS,
                row_for(locale).child(status_pill(
                    locale,
                    palette,
                    label(gui::GUI_GALLERY_READY).into(),
                    ColorToken::StatusRunning,
                    ColorToken::StatusRunningBg,
                )),
            ))
            .child(
                self.card(
                    gui::GUI_GALLERY_PROGRESS,
                    Progress::new("gallery-progress")
                        .value(self.progress)
                        .accessibility_label(label(gui::GUI_GALLERY_PROGRESS)),
                ),
            )
            .child(self.card(
                gui::GUI_GALLERY_EMPTY,
                empty_state(
                    "gallery-empty",
                    locale,
                    palette,
                    label(gui::GUI_GALLERY_NONE),
                ),
            ))
            .child(self.card(
                gui::GUI_GALLERY_ERROR,
                error_state(
                    "gallery-error",
                    locale,
                    palette,
                    label(gui::GUI_GALLERY_FAILED),
                ),
            ))
            .child(
                self.card(
                    gui::GUI_GALLERY_CODE,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Space::S8.pixels()))
                        .child(monospace_block(locale, palette, COMMAND))
                        .child(
                            Button::new("copy-command")
                                .label(label(gui::GUI_GALLERY_COPY))
                                .on_click(|_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(COMMAND.into()))
                                }),
                        ),
                ),
            )
    }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.locale;
        let palette = self.palette;
        let controls = self.controls(cx);
        let data = self.data();
        let feedback = self.feedback(cx);
        let scroll = self.scroll.clone();
        Role::Body
            .with_ui_font(locale == Locale::Arabic, DisplaySettings::default().ui_font)
            .apply(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px(Space::S12.pixels()))
                    .p(px(Space::S24.pixels()))
                    .pt(px(Space::S48.pixels()))
                    .bg(Hsla::from(palette.color(ColorToken::BgCanvas)))
                    .text_color(palette.color(ColorToken::TextPrimary))
                    .child(
                        Role::Title
                            .with_ui_font(
                                locale == Locale::Arabic,
                                DisplaySettings::default().ui_font,
                            )
                            .apply(align_to_start(div(), locale))
                            .child(gui::GUI_GALLERY_TITLE.text(locale)),
                    )
                    .child(
                        div()
                            .id("gallery-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .on_key_down(move |event, window, cx| {
                                if event.keystroke.modifiers.modified() {
                                    return;
                                }
                                let page = match event.keystroke.key.as_str() {
                                    "pageup" => 1.,
                                    "pagedown" => -1.,
                                    _ => return,
                                };
                                let mut offset = scroll.offset();
                                offset.y = (offset.y + scroll.bounds().size.height * page)
                                    .clamp(-scroll.max_offset().y, px(0.));
                                scroll.set_offset(offset);
                                window.refresh();
                                cx.stop_propagation();
                            })
                            .child(
                                row_for(locale)
                                    .items_start()
                                    .gap(px(Space::S12.pixels()))
                                    .child(controls)
                                    .child(data)
                                    .child(feedback),
                            )
                            .vertical_scrollbar(&self.scroll),
                    ),
            )
    }
}

fn column() -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(Space::S12.pixels()))
}

fn open_dialog(
    typed: bool,
    locale: Locale,
    palette: Palette,
    window: &mut Window,
    cx: &mut marsa_ui::gpui::App,
) {
    let confirmation = typed.then(|| TypedConfirmation::new(PHRASE, window, cx));
    let title = if typed {
        gui::GUI_GALLERY_CONFIRMATION
    } else {
        gui::GUI_GALLERY_DIALOG
    };
    window.open_dialog(cx, move |dialog, _, cx| {
        let can_submit = confirmation
            .as_ref()
            .is_none_or(|input| input.confirmed(cx));
        let confirm = confirmation.clone();
        let content = match &confirmation {
            Some(input) => div()
                .flex()
                .flex_col()
                .gap(px(Space::S8.pixels()))
                .child(monospace_block(locale, palette, input.phrase()))
                .child(input.field(gui::GUI_GALLERY_TYPE_CONFIRMATION.text(locale))),
            None => align_to_start(div(), locale).child(gui::GUI_GALLERY_READY.text(locale)),
        };
        dialog_frame(dialog, Space::S80.pixels() * 5.)
            .overlay_closable(false)
            .title(dialog_heading(locale, palette, title.text(locale)))
            .child(content)
            .footer(
                dialog_actions(locale, palette)
                    .child(
                        Button::new("cancel")
                            .label(gui::GUI_GALLERY_DISMISS.text(locale))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("confirm")
                            .primary()
                            .disabled(!can_submit)
                            .label(gui::GUI_GALLERY_ACCEPT.text(locale))
                            .on_click(move |_, window, cx| {
                                if confirm.as_ref().is_none_or(|input| input.confirmed(cx)) {
                                    window.close_dialog(cx);
                                    window.push_notification(
                                        Notification::new()
                                            .message(gui::GUI_GALLERY_SAVED.text(locale)),
                                        cx,
                                    );
                                }
                            }),
                    ),
            )
    });
}
