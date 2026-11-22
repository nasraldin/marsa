//! A native preedit buffer that sends only committed text to the PTY.

use std::ops::Range;

use marsa_client::docker::MAX_TERMINAL_INPUT;
use marsa_ui::gpui::{
    Bounds, Context, EntityInputHandler, Pixels, Point, TextInputConfiguration, UTF16Selection,
    Window, size,
};

use super::{CELL_HEIGHT, ContainerView, cell_width};

#[derive(Default)]
pub(super) struct Composition {
    pub(super) marked: String,
    selection: Range<usize>,
}

impl Composition {
    fn range(&self) -> Range<usize> {
        0..self.marked.encode_utf16().count()
    }
}

impl ContainerView {
    fn commit_terminal_text(&mut self, input: &str, cx: &mut Context<Self>) {
        let Some(session) = self
            .terminal
            .current_mut()
            .filter(|session| session.opened && !session.closed)
        else {
            return;
        };
        let mut text = String::new();
        for ch in input.chars().filter(|ch| !ch.is_control()) {
            if text.len() + ch.len_utf8() > MAX_TERMINAL_INPUT {
                session.input_failed = true;
                cx.notify();
                return;
            }
            text.push(ch);
        }
        if !text.is_empty() {
            session.parser.screen_mut().set_scrollback(0);
            session.input_failed = !session.input.write(text.into_bytes());
        }
        cx.notify();
    }
}

impl EntityInputHandler for ContainerView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let composition = &self.terminal.compose;
        let utf16: Vec<_> = composition.marked.encode_utf16().collect();
        let range = range.start.min(utf16.len())..range.end.min(utf16.len());
        let input = utf16.get(range.clone())?;
        let text = String::from_utf16(input).ok()?;
        *adjusted_range = Some(range);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.terminal.current()?;
        Some(UTF16Selection {
            range: self.terminal.compose.selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let range = self.terminal.compose.range();
        (!range.is_empty()).then_some(range)
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let composition = std::mem::take(&mut self.terminal.compose);
        self.commit_terminal_text(&composition.marked, cx);
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.compose = Composition::default();
        self.commit_terminal_text(text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        selected_range: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.len() > MAX_TERMINAL_INPUT {
            self.terminal.compose = Composition::default();
            cx.notify();
            return;
        }
        let composition = &mut self.terminal.compose;
        composition.marked = text.to_owned();
        let terminal_end = composition.range().end;
        composition.selection = selected_range
            .map(|range| range.start.min(terminal_end)..range.end.min(terminal_end))
            .unwrap_or(terminal_end..terminal_end);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (row, column) = self.terminal.current()?.parser.screen().cursor_position();
        let width = cell_width(window);
        let origin = Point {
            x: bounds.origin.x + width * f32::from(column),
            y: bounds.origin.y + CELL_HEIGHT * f32::from(row),
        };
        Some(Bounds {
            origin,
            size: size(width, CELL_HEIGHT),
        })
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.terminal.compose.selection.end)
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.terminal.compose.range().end)
    }

    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        self.terminal
            .current()
            .is_some_and(|session| session.opened && !session.closed)
    }

    fn text_input_configuration(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> TextInputConfiguration {
        TextInputConfiguration {
            autocorrect: false,
            suggestions: false,
            ..TextInputConfiguration::default()
        }
    }
}
