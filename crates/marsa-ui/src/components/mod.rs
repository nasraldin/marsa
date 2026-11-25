//! Token-themed controls for the workspace and its component gallery.

pub mod dialogs;
pub mod feedback;
pub mod indicators;
pub mod inputs;
pub mod split;

pub use gpui_component::{
    VirtualList, VirtualListScrollHandle, button, checkbox, dialog, list, menu, notification,
    progress, select, switch, tab, table, tooltip, tree, v_virtual_list,
};

use crate::{ColorToken, Palette};

fn color(palette: Palette, token: ColorToken) -> gpui::Hsla {
    palette.color(token).into()
}
