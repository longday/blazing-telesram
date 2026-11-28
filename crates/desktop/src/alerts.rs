//! What the app tells the user from outside its window: today the Dock badge.
//!
//! `Alerts` owns no UI. It reads the unread total of the store (`ChatStore::badge_count`: the unread
//! counters of the chats that are not muted, as the list shows them) and hands the label to the
//! `DockBadge` that the registered `AlertsFactory` makes. The only implementation that touches a
//! platform API is `dock.rs`; tests use the scripted fake of this module. No factory registered
//! means no alerts: the entity does nothing.
//! The badge is recomputed from the store alone: no request is made because of it.
//! Nothing here logs: no label is ever written to a log line.

use crate::store::{ChatStore, StoreEvent};
use gpui::{Context, Entity, Global, Subscription};

/// The largest count the badge shows as a number; above it the label reads «999+».
const BADGE_MAX: u32 = 999;

/// The badge on the app's Dock icon, driven by `Alerts` from the UI thread.
pub trait DockBadge {
    /// Shows `label` on the icon; `None` removes the badge.
    fn set(&mut self, label: Option<&str>);
}

/// What the platform gives the app to alert the user with.
pub struct Platform {
    pub badge: Box<dyn DockBadge>,
}

/// Makes the `Platform` an `Alerts` uses. A gpui global: the app registers the real one, the test
/// harness a scripted fake; no global means "no alerts".
pub struct AlertsFactory(pub fn() -> Platform);

impl Global for AlertsFactory {}

/// The label of the badge for `total` unread messages: `None` at 0 (no badge), the number up to
/// 999, «999+» above.
pub fn badge_label(total: u32) -> Option<String> {
    match total {
        0 => None,
        1..=BADGE_MAX => Some(total.to_string()),
        _ => Some(format!("{BADGE_MAX}+")),
    }
}

pub struct Alerts {
    store: Entity<ChatStore>,
    /// `None` when no factory is registered.
    badge: Option<Box<dyn DockBadge>>,
    /// The label last handed to `badge`.
    label: Option<String>,
    _subscription: Option<Subscription>,
}

impl Alerts {
    pub fn new(store: Entity<ChatStore>, cx: &mut Context<Self>) -> Self {
        let badge = cx
            .try_global::<AlertsFactory>()
            .map(|factory| (factory.0)().badge);
        let subscription = badge.as_ref().map(|_| {
            cx.subscribe(&store, |this, _, event: &StoreEvent, cx| {
                // `Chats` covers a push, a read here or elsewhere and a reload; the store forgets its
                // chats at sign-out and at an account switch, which end in `Session` and `Account`.
                if matches!(
                    event,
                    StoreEvent::Chats | StoreEvent::Session | StoreEvent::Account
                ) {
                    this.sync_badge(cx);
                }
            })
        });
        let mut this = Self {
            store,
            badge,
            label: None,
            _subscription: subscription,
        };
        this.sync_badge(cx);
        this
    }

    /// Hands the platform the label of the store's total, only when it differs from the last one.
    fn sync_badge(&mut self, cx: &Context<Self>) {
        let Some(badge) = self.badge.as_mut() else {
            return;
        };
        let label = badge_label(self.store.read(cx).badge_count());
        if label != self.label {
            badge.set(label.as_deref());
            self.label = label;
        }
    }
}

impl Drop for Alerts {
    /// The badge never outlives the window that owns this entity.
    fn drop(&mut self) {
        if let Some(badge) = self.badge.as_mut() {
            badge.set(None);
        }
    }
}

/// A scripted `DockBadge` for tests: the factory is a plain `fn`, so the log lives in a
/// thread-local (a gpui test runs on one thread) and `Fake` is the handle to it.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use gpui::App;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Script {
        calls: Vec<Option<String>>,
        created: usize,
    }

    thread_local! {
        static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
    }

    fn script<R>(f: impl FnOnce(&mut Script) -> R) -> R {
        SCRIPT.with(|s| f(&mut s.borrow_mut()))
    }

    /// The handle to the log of this thread's fake badge.
    pub struct Fake;

    /// Resets the log: nothing was called, no badge was made.
    pub fn reset() -> Fake {
        script(|s| *s = Script::default());
        Fake
    }

    /// Registers the fake as the app's alerts factory.
    pub fn install(cx: &mut App) {
        cx.set_global(AlertsFactory(|| {
            script(|s| s.created += 1);
            Platform {
                badge: Box::new(Badge),
            }
        }));
    }

    impl Fake {
        /// The labels handed over by `set` since the last take, in order (`None`: the badge removed).
        pub fn take_calls(&self) -> Vec<Option<String>> {
            script(|s| std::mem::take(&mut s.calls))
        }

        /// How many platforms the factory made.
        pub fn created(&self) -> usize {
            script(|s| s.created)
        }
    }

    struct Badge;

    impl DockBadge for Badge {
        fn set(&mut self, label: Option<&str>) {
            script(|s| s.calls.push(label.map(str::to_owned)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{self, Fake};
    use super::*;
    use async_channel::Sender;
    use bt_chat_core::model::{ChatId, ChatKind, ChatSummary, ClientEvent};
    use gpui::{AppContext, TestAppContext};

    #[test]
    fn labels() {
        assert_eq!(badge_label(0), None);
        assert_eq!(badge_label(1), Some("1".into()));
        assert_eq!(badge_label(999), Some("999".into()));
        assert_eq!(badge_label(1000), Some("999+".into()));
        assert_eq!(badge_label(u32::MAX), Some("999+".into()));
    }

    fn row(tag: &str, unread: u32, muted: bool) -> ChatSummary {
        ChatSummary {
            id: ChatId(tag.into()),
            kind: ChatKind::Private,
            title: format!("chat {tag}"),
            avatar: None,
            last_message: None,
            unread,
            muted,
            pinned_rank: None,
            partner: None,
            pinned_message: None,
            pin_action_ts: None,
        }
    }

    fn some(label: &str) -> Option<String> {
        Some(label.into())
    }

    struct Rig {
        store: Entity<ChatStore>,
        fake: Fake,
        alerts: Entity<Alerts>,
        _client: Sender<ClientEvent>,
    }

    impl Rig {
        fn new(cx: &mut TestAppContext) -> Self {
            let fake = fake::reset();
            cx.update(fake::install);
            Self::build(cx, fake)
        }

        /// A rig with no factory registered.
        fn inert(cx: &mut TestAppContext) -> Self {
            Self::build(cx, fake::reset())
        }

        fn build(cx: &mut TestAppContext, fake: Fake) -> Self {
            let (client, event_rx) = async_channel::unbounded();
            let (command_tx, _commands) = async_channel::unbounded();
            let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
            let alerts = cx.new(|cx| Alerts::new(store.clone(), cx));
            Self {
                store,
                fake,
                alerts,
                _client: client,
            }
        }

        fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
            self.store.update(cx, |s, cx| s.apply(event, cx));
        }

        fn list(&self, cx: &mut TestAppContext, rows: Vec<ChatSummary>) {
            self.apply(cx, ClientEvent::ChatsReplaced(rows));
        }
    }

    #[gpui::test]
    fn a_first_list_sets_the_label_of_the_unmuted_total(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        assert_eq!(rig.fake.created(), 1);
        assert_eq!(rig.fake.take_calls(), [], "an empty store sets nothing");

        rig.list(
            cx,
            vec![row("a", 3, false), row("b", 4, false), row("m", 120, true)],
        );
        assert_eq!(rig.fake.take_calls(), [some("7")]);
    }

    #[gpui::test]
    fn an_equal_total_does_not_call_again(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.list(cx, vec![row("a", 3, false), row("b", 4, false)]);
        assert_eq!(rig.fake.take_calls(), [some("7")]);

        // The same list again, a row that changes nothing, a split of the same total.
        rig.list(cx, vec![row("a", 3, false), row("b", 4, false)]);
        rig.apply(cx, ClientEvent::ChatUpdated(row("a", 3, false)));
        rig.list(cx, vec![row("a", 5, false), row("b", 2, false)]);
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn a_change_of_the_total_calls_once_and_a_read_clears(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.list(cx, vec![row("a", 2, false), row("b", 0, false)]);
        rig.apply(cx, ClientEvent::ChatUpdated(row("b", 1, false)));
        assert_eq!(rig.fake.take_calls(), [some("2"), some("3")]);

        // Read elsewhere: lowers.
        rig.apply(cx, ClientEvent::ChatUpdated(row("a", 0, false)));
        assert_eq!(rig.fake.take_calls(), [some("1")]);

        rig.apply(cx, ClientEvent::ChatUpdated(row("b", 0, false)));
        assert_eq!(rig.fake.take_calls(), [None]);
    }

    #[gpui::test]
    fn a_muted_arrival_does_not_call(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.list(cx, vec![row("a", 2, false), row("m", 0, true)]);
        assert_eq!(rig.fake.take_calls(), [some("2")]);

        rig.apply(cx, ClientEvent::ChatUpdated(row("m", 1, true)));
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn a_large_total_reads_999_plus(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.list(cx, vec![row("a", 1000, false), row("b", 500, false)]);
        assert_eq!(rig.fake.take_calls(), [some("999+")]);
        // Still above the cap: the label is the same.
        rig.apply(cx, ClientEvent::ChatUpdated(row("b", 700, false)));
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn sign_out_and_account_switch_clear_the_badge(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.list(cx, vec![row("a", 7, false)]);
        rig.apply(cx, ClientEvent::SignedOut);
        assert_eq!(rig.fake.take_calls(), [some("7"), None]);

        rig.list(cx, vec![row("a", 7, false)]);
        rig.apply(cx, ClientEvent::AccountSwitched);
        assert_eq!(rig.fake.take_calls(), [some("7"), None]);
    }

    #[gpui::test]
    fn dropping_the_entity_clears_the_badge(cx: &mut TestAppContext) {
        let Rig {
            store,
            fake,
            alerts,
            _client,
        } = Rig::new(cx);
        store.update(cx, |s, cx| {
            s.apply(ClientEvent::ChatsReplaced(vec![row("a", 7, false)]), cx)
        });
        assert_eq!(fake.take_calls(), [some("7")]);

        drop(alerts);
        cx.update(|_| {});
        assert_eq!(fake.take_calls(), [None]);

        // The store lives on and no longer reaches the badge.
        store.update(cx, |s, cx| {
            s.apply(ClientEvent::ChatsReplaced(vec![row("a", 9, false)]), cx)
        });
        assert_eq!(fake.take_calls(), []);
    }

    #[gpui::test]
    fn a_store_that_already_holds_chats_sets_the_label_at_creation(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.list(cx, vec![row("a", 4, false)]);
        rig.fake.take_calls();

        let store = rig.store.clone();
        let _second = cx.new(|cx| Alerts::new(store, cx));
        assert_eq!(rig.fake.take_calls(), [some("4")]);
        assert_eq!(rig.fake.created(), 2);
    }

    #[gpui::test]
    fn no_factory_means_no_call_and_no_panic(cx: &mut TestAppContext) {
        let rig = Rig::inert(cx);
        rig.list(cx, vec![row("a", 7, false)]);
        rig.apply(cx, ClientEvent::SignedOut);
        assert_eq!(rig.fake.created(), 0);
        assert_eq!(rig.fake.take_calls(), []);

        let Rig { alerts, fake, .. } = rig;
        drop(alerts);
        cx.update(|_| {});
        assert_eq!(fake.take_calls(), []);
    }
}
