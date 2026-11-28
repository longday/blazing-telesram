//! The real `DockBadge`: the badge label of the app's Dock tile (`NSDockTile`).
//!
//! `NSDockTile.badgeLabel` is an app-owned label: it needs neither a bundle nor a notification
//! permission. The tile is taken once, on the main thread, when the badge is made; every call of
//! the bindings used here is safe, so this file holds no `unsafe`.
//! Nothing here logs: the label is never written to a log line.

use crate::alerts::DockBadge;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSDockTile};
use objc2_foundation::NSString;

/// The badge of the app's Dock icon, for the `AlertsFactory` of the app. It does nothing when it is
/// made off the main thread (AppKit allows the Dock tile there only); gpui calls the factory on it.
pub fn badge() -> Box<dyn DockBadge> {
    let tile = MainThreadMarker::new().map(|mtm| NSApplication::sharedApplication(mtm).dockTile());
    if tile.is_none() {
        tracing::warn!("dock badge unavailable: not on the main thread");
    }
    Box::new(Dock { tile })
}

struct Dock {
    tile: Option<Retained<NSDockTile>>,
}

/// The string handed to the bindings: `None` removes the badge.
fn ns_label(label: Option<&str>) -> Option<Retained<NSString>> {
    label.map(NSString::from_str)
}

impl DockBadge for Dock {
    fn set(&mut self, label: Option<&str>) {
        if let Some(tile) = &self.tile {
            tile.setBadgeLabel(ns_label(label).as_deref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_reaches_the_bindings_as_the_same_text() {
        assert_eq!(ns_label(Some("7")).map(|s| s.to_string()), Some("7".into()));
        assert_eq!(
            ns_label(Some("999+")).map(|s| s.to_string()),
            Some("999+".into())
        );
        assert_eq!(ns_label(None), None);
    }

    #[test]
    fn off_the_main_thread_the_badge_does_nothing() {
        // A test thread is not the main thread, so there is no tile to touch.
        assert!(MainThreadMarker::new().is_none());
        let mut badge = badge();
        badge.set(Some("7"));
        badge.set(None);
    }
}
