//! Small shared elements.

use crate::format::{avatar_color, initials};
use crate::store::ChatStore;
use crate::theme::*;
use bt_chat_core::model::MediaRef;
use gpui::{prelude::*, *};

/// Round avatar: the downloaded image when ready, otherwise initials on a color derived from `key`.
/// Debug selectors: `tag`, plus `tag-image` or `tag-initials` for what is shown.
pub fn avatar(
    store: &ChatStore,
    media: Option<&MediaRef>,
    key: &str,
    label: &str,
    size: f32,
    tag: String,
) -> Div {
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
        None => base.child(
            div()
                .debug_selector({
                    let tag = tag.clone();
                    move || format!("{tag}-initials")
                })
                .child(initials(label)),
        ),
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

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Reply,
    Copy,
    Edit,
    Trash,
}

/// A polyline on the icon grid; `closed` joins the last point to the first.
type Stroke = (bool, &'static [(f32, f32)]);

impl Icon {
    /// Strokes on a 16×16 grid. The reply arrow's curved tail is added separately.
    fn strokes(self) -> &'static [Stroke] {
        match self {
            Self::Reply => &[(false, &[(6.5, 3.5), (2.5, 7.5), (6.5, 11.5)])],
            Self::Copy => &[
                (true, &[(5.5, 5.5), (13., 5.5), (13., 13.), (5.5, 13.)]),
                (false, &[(10.5, 3.), (3., 3.), (3., 10.5)]),
            ],
            Self::Edit => &[
                (
                    true,
                    &[
                        (2.5, 13.5),
                        (3.2, 10.),
                        (10.8, 2.4),
                        (13.6, 5.2),
                        (6., 12.8),
                    ],
                ),
                (false, &[(9.2, 4.), (12., 6.8)]),
            ],
            Self::Trash => &[
                (false, &[(2.5, 4.5), (13.5, 4.5)]),
                (false, &[(6., 4.5), (6., 2.5), (10., 2.5), (10., 4.5)]),
                (false, &[(4., 4.5), (4.8, 13.5), (11.2, 13.5), (12., 4.5)]),
            ],
        }
    }
}

/// A square line icon drawn as a stroked path, so it does not depend on an emoji font.
pub fn icon(kind: Icon, size: f32, color: u32) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let unit = size / 16.;
            let at = |(x, y): (f32, f32)| {
                point(
                    bounds.origin.x + px(x * unit),
                    bounds.origin.y + px(y * unit),
                )
            };
            let mut builder = PathBuilder::stroke(px(1.5 * unit));
            for &(closed, stroke) in kind.strokes() {
                let points: Vec<_> = stroke.iter().copied().map(at).collect();
                builder.add_polygon(&points, closed);
            }
            if matches!(kind, Icon::Reply) {
                builder.move_to(at((2.5, 7.5)));
                builder.line_to(at((9., 7.5)));
                builder.curve_to(at((13.5, 12.)), at((13.5, 7.5)));
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, Hsla::from(rgb(color)));
            }
        },
    )
    .size(px(size))
    .flex_none()
}
