//! Small shared elements.

use crate::format::{avatar_color, initials};
use crate::store::ChatStore;
use crate::theme::*;
use bt_chat_core::model::MediaRef;
use gpui::{prelude::*, *};

/// Round avatar: the downloaded image when ready, otherwise initials on a color derived from `key`.
/// Debug selectors: `tag`, plus `tag-image` or `tag-initials` for what is shown.
pub fn avatar(store: &ChatStore, media: Option<&MediaRef>, key: &str, label: &str, size: f32, tag: String) -> Div {
    let base = div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(rgb(avatar_color(key)))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(0xffffff))
        .text_size(px(size * 0.38))
        .font_weight(FontWeight::MEDIUM);
    match media.and_then(|m| store.media_path(m)) {
        Some(path) => base.child(
            img(path.to_path_buf())
                .object_fit(ObjectFit::Cover)
                .size(px(size))
                .rounded_full()
                .debug_selector({
                    let tag = tag.clone();
                    move || format!("{tag}-image")
                }),
        ),
        None => base.child(div().debug_selector({
            let tag = tag.clone();
            move || format!("{tag}-initials")
        }).child(initials(label))),
    }
    .debug_selector(move || tag)
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(8.))
        .rounded(px(8.))
        .text_size(px(14.))
        .cursor_pointer()
        .bg(rgb(FIELD))
        .hover(|s| s.bg(rgb(ROW_HOVER)))
        .child(label.into())
}
