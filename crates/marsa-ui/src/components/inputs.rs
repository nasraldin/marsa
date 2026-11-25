//! Text controls shared by desktop forms.

use gpui::{App, AppContext as _, Entity, SharedString, Styled, Window, px};
use gpui_component::{
    IconName,
    input::{Input, InputEvent, InputState},
};

use crate::tokens::scale::Space;

/// Exact-text input for an action that requires a fresh confirmation.
#[derive(Clone)]
pub struct TypedConfirmation {
    expected: SharedString,
    input: Entity<InputState>,
}

impl TypedConfirmation {
    /// Start with a blank input and the offered confirmation phrase.
    pub fn new(expected: impl Into<SharedString>, window: &mut Window, cx: &mut App) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&input, |_, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.refresh_windows();
            }
        })
        .detach();
        Self {
            expected: expected.into(),
            input,
        }
    }

    /// Text to display to the person reviewing the action.
    pub fn phrase(&self) -> &str {
        self.expected.as_ref()
    }

    /// Render the text input with the caller's localized accessible label.
    pub fn field(&self, label: impl Into<SharedString>) -> Input {
        resource_input(&self.input).aria_label(label)
    }

    /// Check the current value again immediately before committing the action.
    pub fn confirmed(&self, cx: &App) -> bool {
        confirmation_matches(self.phrase(), self.input.read(cx).value().as_ref())
    }
}

fn confirmation_matches(expected: &str, value: &str) -> bool {
    !expected.is_empty() && value == expected
}

/// Build a standard-height text input for a Marsa form.
pub fn resource_input(state: &Entity<InputState>) -> Input {
    Styled::h(Input::new(state), px(Space::S36.pixels()))
}

/// Build a labeled search input with a search icon and a clear action.
pub fn search_input(state: &Entity<InputState>, label: impl Into<SharedString>) -> Input {
    resource_input(state)
        .cleanable(true)
        .prefix(IconName::Search)
        .aria_label(label)
}

/// Keep a retained input placeholder synchronized after a locale change.
pub fn update_input_placeholder(
    state: &Entity<InputState>,
    value: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    if state.read(cx).presentation().placeholder() != value {
        state.update(cx, |state, cx| state.set_placeholder(value, window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::confirmation_matches;

    #[test]
    fn accepts_only_a_nonempty_exact_match() {
        assert!(confirmation_matches("volume-ألف", "volume-ألف"));
        assert!(!confirmation_matches("volume-ألف", "volume-ألف "));
        assert!(!confirmation_matches("Volume-ألف", "volume-ألف"));
        assert!(!confirmation_matches("", ""));
    }
}
