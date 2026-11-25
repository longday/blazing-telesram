//! Image viewer: a full-window overlay over the chat that shows one picture of a message at a time.
//!
//! `Desktop` owns the `Viewer` (which message, which picture) and rebuilds the overlay from the store on
//! every render, so it follows live edits and download progress. The picture is the cached preview; the
//! original is only fetched when the user chooses Download.

use crate::desktop::Desktop;
use crate::format::human_size;
use crate::store::{ChatStore, DownloadState};
use crate::theme::*;
use crate::widgets::button;
use bt_chat_core::model::{ChatId, ConnectionState, DownloadFailure, GalleryItem, MessageTs};
use gpui::{prelude::*, *};
use std::path::PathBuf;

/// The longest side of the picture box; smaller pictures keep their size.
const IMAGE_MAX: f32 = 800.;
/// Fixed bands around the picture, so the space left for it follows from the window size alone.
const TOP_BAR: f32 = 56.;
const BOTTOM_BAR: f32 = 64.;
const PAD: f32 = 16.;
const ARROW_WIDTH: f32 = 48.;

/// The picture box for a `width`x`height` picture in a window of `viewport` (width, height): the picture
/// scaled down, never up, to the space left after the bars, the padding and (with several pictures) the
/// two arrows, and to `IMAGE_MAX`. Unknown dimensions give a square.
pub fn picture_box(width: u32, height: u32, viewport: (f32, f32), multiple: bool) -> (f32, f32) {
    let arrows = if multiple {
        2. * (ARROW_WIDTH + PAD)
    } else {
        0.
    };
    let max_w = (viewport.0 - 2. * PAD - arrows).clamp(1., IMAGE_MAX);
    let max_h = (viewport.1 - TOP_BAR - BOTTOM_BAR - 2. * PAD).clamp(1., IMAGE_MAX);
    if width == 0 || height == 0 {
        let side = 0.6 * max_w.min(max_h);
        return (side, side);
    }
    let (w, h) = (width as f32, height as f32);
    let scale = (max_w / w).min(max_h / h).min(1.);
    (w * scale, h * scale)
}

/// Which picture of which message the viewer shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Viewer {
    pub chat: ChatId,
    pub ts: MessageTs,
    pub index: usize,
}

/// `index` moved by `delta` and clamped to `0..len`; the ends do not wrap.
pub fn step(index: usize, len: usize, delta: isize) -> usize {
    index
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

/// One-based position among the message's pictures, e.g. `3 / 4`.
pub fn position_label(index: usize, len: usize) -> String {
    format!("{} / {len}", index + 1)
}

/// Words for a failed download; the file card shows the same text.
pub fn failure_text(reason: DownloadFailure) -> &'static str {
    match reason {
        DownloadFailure::Offline => "Нет соединения",
        DownloadFailure::NotFound => "Файл больше недоступен",
        DownloadFailure::Destination => "Папка «Загрузки» недоступна",
        DownloadFailure::Disk => "Не удалось записать на диск",
        DownloadFailure::Network => "Ошибка сети",
        DownloadFailure::Other => "Не удалось скачать",
    }
}

/// The button of the download line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Download,
    Cancel,
    Retry,
    DownloadAgain,
}

impl Action {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Action::Download => "Скачать",
            Action::Cancel => "Отмена",
            Action::Retry => "Повторить",
            Action::DownloadAgain => "Скачать ещё раз",
        }
    }

    fn selector(self) -> &'static str {
        match self {
            Action::Download | Action::DownloadAgain => "viewer-download",
            Action::Cancel => "viewer-cancel",
            Action::Retry => "viewer-retry",
        }
    }
}

/// What the download line under the picture says and offers for a download state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionLine {
    pub status: Option<String>,
    pub action: Action,
    /// Starting a transfer needs the connection; cancelling does not.
    pub enabled: bool,
}

pub fn action_line(state: Option<&DownloadState>, online: bool) -> ActionLine {
    let (status, action) = match state {
        None if online => (None, Action::Download),
        None => (Some("Нет соединения".to_string()), Action::Download),
        Some(DownloadState::Running { received, total }) => {
            let progress = match total {
                Some(total) => format!("{} из {}", human_size(*received), human_size(*total)),
                None => human_size(*received),
            };
            (Some(progress), Action::Cancel)
        }
        Some(DownloadState::Saved(path)) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (Some(format!("Сохранено: {name}")), Action::DownloadAgain)
        }
        Some(DownloadState::Failed(reason)) => {
            (Some(failure_text(*reason).to_string()), Action::Retry)
        }
    };
    ActionLine {
        enabled: online || action == Action::Cancel,
        status,
        action,
    }
}

/// Everything one frame of the overlay needs, read from the store up front so building the element
/// holds no borrow of it.
pub struct Frame {
    item: GalleryItem,
    index: usize,
    len: usize,
    picture: (f32, f32),
    preview: Option<PathBuf>,
    line: ActionLine,
}

/// `viewport` is the window's (width, height).
pub fn frame(
    viewer: &Viewer,
    items: &[GalleryItem],
    store: &ChatStore,
    viewport: (f32, f32),
) -> Option<Frame> {
    let item = items.get(viewer.index)?.clone();
    let online = store.connection() == ConnectionState::Online;
    Some(Frame {
        picture: picture_box(item.width, item.height, viewport, items.len() > 1),
        preview: store.media_path(&item.media).map(PathBuf::from),
        line: action_line(store.download_state(&item.media.original()), online),
        index: viewer.index,
        len: items.len(),
        item,
    })
}

pub fn render(frame: Frame, cx: &mut Context<Desktop>) -> Div {
    let Frame {
        item,
        index,
        len,
        picture: (w, h),
        preview,
        line,
    } = frame;
    let position = position_label(index, len);
    let position_selector = format!("viewer-pos-{}-{len}", index + 1);
    let multiple = len > 1;

    let picture = div()
        .debug_selector(|| "viewer-image".into())
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .rounded(px(8.))
        .bg(rgb(PLACEHOLDER))
        .when_some(preview, |d, path| {
            d.child(
                img(path)
                    .object_fit(ObjectFit::Contain)
                    .size_full()
                    .rounded(px(8.))
                    .debug_selector(|| "viewer-image-loaded".into()),
            )
        });

    let original = item.media.original();
    let name = item.name.clone();
    let action = line.action;
    let mut act =
        button(action.selector(), action.label()).debug_selector(move || action.selector().into());
    act = if line.enabled {
        act.on_click(cx.listener(move |this, _, _, cx| match action {
            Action::Cancel => this.store.read(cx).cancel_download(&original),
            Action::Download | Action::Retry | Action::DownloadAgain => {
                this.store.update(cx, |store, cx| {
                    store.download(original.clone(), name.clone(), cx)
                });
            }
        }))
    } else {
        act.cursor_default().text_color(rgb(MUTED))
    };

    div()
        .debug_selector(|| "viewer".into())
        .absolute()
        .size_full()
        .occlude()
        .flex()
        .flex_col()
        .bg(rgba(0x000000dd))
        .overflow_hidden()
        .child(
            div()
                .h(px(56.))
                .flex_none()
                .px(px(16.))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .debug_selector(move || position_selector)
                        .flex_1()
                        .child(position),
                )
                .child(
                    button("viewer-close", "Закрыть")
                        .debug_selector(|| "viewer-close".into())
                        .on_click(cx.listener(|this, _, _, cx| this.close_viewer(cx))),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .p(px(PAD))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(PAD))
                .when(multiple, |d| {
                    d.child(arrow("viewer-prev", "‹", index > 0, -1, cx))
                })
                .child(picture)
                .when(multiple, |d| {
                    d.child(arrow("viewer-next", "›", index + 1 < len, 1, cx))
                }),
        )
        .child(
            div()
                .flex_none()
                .h(px(BOTTOM_BAR))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .children(line.status.map(|text| {
                    let saved = matches!(line.action, Action::DownloadAgain);
                    div()
                        .debug_selector(move || {
                            String::from(if saved {
                                "viewer-saved"
                            } else {
                                "viewer-status"
                            })
                        })
                        .text_color(rgb(MUTED))
                        .child(text)
                }))
                .child(act),
        )
}

/// Previous/next control; dimmed and inert at the end it cannot move past.
fn arrow(
    id: &'static str,
    glyph: &'static str,
    enabled: bool,
    delta: isize,
    cx: &mut Context<Desktop>,
) -> Stateful<Div> {
    let arrow = button(id, glyph)
        .debug_selector(move || id.into())
        .text_size(px(28.))
        .w(px(ARROW_WIDTH))
        .px(px(0.))
        .flex()
        .justify_center()
        .flex_none();
    if enabled {
        arrow.on_click(cx.listener(move |this, _, _, cx| this.step_viewer(delta, cx)))
    } else {
        arrow.cursor_default().text_color(rgb(MUTED))
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, ActionLine, action_line, picture_box, position_label, step};
    use crate::store::DownloadState;
    use bt_chat_core::model::DownloadFailure;
    use std::path::Path;

    #[test]
    fn step_clamps_at_both_ends() {
        assert_eq!(step(0, 4, -1), 0);
        assert_eq!(step(2, 4, 1), 3);
        assert_eq!(step(3, 4, 1), 3);
        assert_eq!(step(3, 4, -1), 2);
        assert_eq!(step(0, 1, 1), 0);
        assert_eq!(step(0, 0, 1), 0);
        assert_eq!(step(2, 4, isize::MIN), 0);
        assert_eq!(step(2, 4, isize::MAX), 3);
    }

    #[test]
    fn position_is_one_based() {
        assert_eq!(position_label(2, 4), "3 / 4");
        assert_eq!(position_label(0, 1), "1 / 1");
    }

    #[test]
    fn the_action_line_follows_the_download_state() {
        let line = |state: Option<DownloadState>, online| action_line(state.as_ref(), online);
        assert_eq!(
            line(None, true),
            ActionLine {
                status: None,
                action: Action::Download,
                enabled: true
            }
        );
        assert_eq!(
            line(None, false),
            ActionLine {
                status: Some("Нет соединения".into()),
                action: Action::Download,
                enabled: false
            }
        );
        let running = Some(DownloadState::Running {
            received: 1536,
            total: Some(10 * 1024 * 1024),
        });
        assert_eq!(
            line(running, true),
            ActionLine {
                status: Some("1,5 КБ из 10 МБ".into()),
                action: Action::Cancel,
                enabled: true
            }
        );
        let unknown = Some(DownloadState::Running {
            received: 100,
            total: None,
        });
        assert_eq!(
            line(unknown.clone(), false).status.as_deref(),
            Some("100 Б")
        );
        assert!(
            line(unknown, false).enabled,
            "a running download can be cancelled offline"
        );
        let saved = Some(DownloadState::Saved(
            Path::new("/Users/x/Downloads/a (1).jpg").to_path_buf(),
        ));
        assert_eq!(
            line(saved, true),
            ActionLine {
                status: Some("Сохранено: a (1).jpg".into()),
                action: Action::DownloadAgain,
                enabled: true
            }
        );
        let failed = Some(DownloadState::Failed(DownloadFailure::Disk));
        assert_eq!(
            line(failed.clone(), true),
            ActionLine {
                status: Some("Не удалось записать на диск".into()),
                action: Action::Retry,
                enabled: true
            }
        );
        assert!(!line(failed, false).enabled);
    }

    fn near(actual: (f32, f32), expected: (f32, f32)) {
        assert!(
            (actual.0 - expected.0).abs() < 0.01 && (actual.1 - expected.1).abs() < 0.01,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn the_picture_box_fits_the_space_left_by_the_bars_and_arrows() {
        // The test window keeps the 800 cap and the aspect ratio.
        near(picture_box(1600, 800, (1200., 1400.), true), (800., 400.));
        // The smallest window, 850x560, leaves 690x408 with arrows and 818x408 without (the cap is 800).
        near(picture_box(3000, 4000, (850., 560.), true), (306., 408.));
        near(picture_box(4000, 1000, (850., 560.), true), (690., 172.5));
        near(picture_box(4000, 1000, (850., 560.), false), (800., 200.));
        // Small pictures are not scaled up; unknown sizes give a square inside the space.
        near(picture_box(100, 50, (850., 560.), true), (100., 50.));
        near(picture_box(0, 0, (850., 560.), true), (244.8, 244.8));
        // A window smaller than the bars never gives a zero or negative box.
        let (w, h) = picture_box(1000, 1000, (50., 50.), true);
        assert!(w >= 1. && h >= 1.);
    }
}
