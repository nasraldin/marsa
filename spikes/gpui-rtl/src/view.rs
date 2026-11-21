//! Small bilingual layout shared by the interactive window and Metal capture.

use gpui_kit::base::{LayoutDirection, LogicalStyledExt as _, StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{
    ActiveTheme as _,
    button::Button,
    input::{Input, InputState},
};
use gpui_kit::{
    AppContext as _, ClickEvent, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};

pub const ARABIC_FAMILY: &str = "IBM Plex Sans Arabic";
pub const ARABIC_TITLE: &str = "الحاويات";
pub const NAME: &str = "web-api";
pub const IMAGE: &str = "ghcr.io/acme/api:1.4.2";
pub const PATH: &str = "/work/app";

/// Font sizes, line heights, and weights in the Arabic Figma specimen.
pub const TYPE_SCALE: &[(f32, f32, FontWeight)] = &[
    (64., 72., FontWeight::SEMIBOLD),
    (40., 52., FontWeight::SEMIBOLD),
    (28., 40., FontWeight::SEMIBOLD),
    (20., 32., FontWeight::SEMIBOLD),
    (16., 28., FontWeight::MEDIUM),
    (14., 24., FontWeight::NORMAL),
    (13., 24., FontWeight::MEDIUM),
    (12., 20., FontWeight::NORMAL),
    (11., 20., FontWeight::MEDIUM),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Arabic,
}

impl Language {
    pub fn text(self, english: &'static str, arabic: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Arabic => arabic,
        }
    }

    fn code(self) -> &'static str {
        self.text("en-US", "ar")
    }

    fn placeholder(self) -> &'static str {
        self.text("Search containers", "ابحث في الحاويات")
    }
}

pub struct Spike {
    pub language: Language,
    pub input: Entity<InputState>,
}

impl Spike {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let language = Language::English;
        LayoutDirection::from_locale(language.code()).set_global(cx);
        Self {
            language,
            input: cx.new(|cx| InputState::new(window, cx).placeholder(language.placeholder())),
        }
    }

    fn switch_language(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.language = match self.language {
            Language::English => Language::Arabic,
            Language::Arabic => Language::English,
        };
        LayoutDirection::from_locale(self.language.code()).set_global(cx);
        self.input.update(cx, |input, cx| {
            input.set_placeholder(self.language.placeholder(), window, cx);
        });
        cx.notify();
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let strings = self.language;
        div()
            .id("sidebar")
            .test_support()
            .w(px(156.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_3()
            .ps(px(20.), cx)
            .pe(px(16.), cx)
            .py_4()
            .border_e(px(1.), cx)
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .text_size(px(20.))
                    .font_semibold()
                    .child(strings.text("Marsa", "مرسى")),
            )
            .child(strings.text("Containers", ARABIC_TITLE))
            .child(strings.text("Images", "الصور"))
            .child(strings.text("Volumes", "وحدات التخزين"))
    }

    fn container_row(&self, cx: &Context<Self>) -> impl IntoElement {
        let strings = self.language;
        div()
            .id("container-row")
            .test_support()
            .flex_row_dir(cx)
            .gap_4()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .child(cell("name", strings.text("Container", "الحاوية"), NAME))
            .child(cell("image", strings.text("Image", "الصورة"), IMAGE))
            .child(cell("path", strings.text("Path", "المسار"), PATH))
    }
}

impl Render for Spike {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = self.language;
        let family = strings.text(".AppleSystemUIFont", ARABIC_FAMILY);
        div()
            .size_full()
            .flex_row_dir(cx)
            .text_start(cx)
            .font_family(family)
            .text_size(px(14.))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.sidebar(cx))
            .child(
                div()
                    .id("content")
                    .test_support()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(
                        div()
                            .flex_row_dir(cx)
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(28.))
                                    .line_height(px(40.))
                                    .font_semibold()
                                    .child(strings.text("Containers", ARABIC_TITLE)),
                            )
                            .child(
                                Button::new("language-toggle")
                                    .label(strings.text("العربية", "English"))
                                    .on_click(cx.listener(Self::switch_language)),
                            ),
                    )
                    .child(Input::new(&self.input).font_family(family))
                    .child(self.container_row(cx))
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(24.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(mixed_sentence(strings)),
                    )
                    .children(TYPE_SCALE.iter().map(|&(font_size, height, weight)| {
                        div()
                            .font_family(family)
                            .text_size(px(font_size))
                            .line_height(px(height))
                            .font_weight(weight)
                            .child(strings.text("Inspect container", "فحص الحاوية"))
                    })),
            )
    }
}

fn cell(id: &'static str, label: &'static str, value: &'static str) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .flex_1()
        .flex()
        .flex_col()
        .child(label)
        .child(
            div()
                .font_family("Menlo")
                .text_size(px(12.))
                .child(isolate(value)),
        )
}

fn isolate(value: &str) -> SharedString {
    format!("\u{2066}{value}\u{2069}").into()
}

pub fn mixed_sentence(language: Language) -> SharedString {
    let prefix = language.text("Container", "الحاوية");
    let image_label = language.text("Image", "الصورة");
    let path_label = language.text("Path", "المسار");
    format!(
        "{prefix}: {} · {image_label}: {} · {path_label}: {}",
        isolate(NAME),
        isolate(IMAGE),
        isolate(PATH)
    )
    .into()
}
