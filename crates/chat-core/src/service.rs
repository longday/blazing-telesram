//! Adapter boundary for the offline meeting-chat workflow.
use super::{Chat, Core};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::{Path, PathBuf}};

pub trait ChatService {
    fn chats(&self) -> &[Chat];
    fn selected(&self) -> usize;
    fn active(&self) -> &Chat { &self.chats()[self.selected()] }
    fn select(&mut self, index: usize);
    fn search(&self, query: &str, unread_only: bool) -> Vec<usize>;
    fn older(&mut self) -> usize;
    fn save_draft(&mut self, body: &str);
    fn take_thread_draft(&mut self, parent: usize) -> String;
    fn save_thread_draft(&mut self, parent: usize, body: &str);
    fn toggle_muted(&mut self);
    fn send(&mut self) -> bool;
    fn reply(&mut self, parent: usize, body: &str) -> bool;
    fn storage_status(&self) -> &'static str;
}

#[derive(Serialize, Deserialize)]
enum Event {
    Send { chat: usize, body: String },
    Reply { chat: usize, parent: usize, body: String },
}

/// Local-only adapter. Stores sent mock messages/answers, never credentials or session state.
pub struct LocalChatService {
    core: Core,
    path: Option<PathBuf>,
    events: Vec<Event>,
    storage_error: bool,
}
impl LocalChatService {
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut service = Self { core: Core::new(), path, events: Vec::new(), storage_error: false };
        if let Some(path) = service.path.as_deref() {
            match fs::read(path) {
                Ok(bytes) => match serde_json::from_slice::<Vec<Event>>(&bytes) {
                    Ok(events) => {
                        let unread: Vec<_> = service.core.chats.iter().map(|c| c.unread).collect();
                        for event in &events { service.replay(event); }
                        service.core.select(0);
                        for (c, n) in service.core.chats.iter_mut().zip(unread) { c.unread = n; }
                        service.events = events;
                    }
                    Err(_) => service.storage_error = true,
                },
                Err(e) if e.kind() == io::ErrorKind::NotFound => {},
                Err(_) => service.storage_error = true,
            }
        }
        service
    }
    fn replay(&mut self, event: &Event) {
        let index = match event { Event::Send {chat,..} | Event::Reply {chat,..} => *chat };
        if index >= self.core.chats.len() { return; }
        self.core.select(index);
        match event {
            Event::Send {body,..} => {
                self.save_draft(body);
                self.core.send();
            }
            Event::Reply {parent,body,..} => {
                while self.core.active().oldest > *parent && self.core.load_older() > 0 {}
                self.core.reply(*parent,body);
            }
        }
    }
    fn record(&mut self, event: Event) {
        self.events.push(event);
        if self.storage_error { return; }
        if let Some(path) = self.path.as_deref() {
            self.storage_error = write_events(path, &self.events).is_err();
        }
    }
}
fn write_events(path: &Path, events: &[Event]) -> io::Result<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let temp = path.with_extension("tmp");
    fs::write(&temp,serde_json::to_vec(events)?)?;
    fs::rename(temp, path)
}
impl ChatService for LocalChatService {
    fn chats(&self) -> &[Chat] { &self.core.chats }
    fn selected(&self) -> usize { self.core.selected }
    fn select(&mut self, index: usize) { self.core.select(index) }
    fn search(&self, query: &str, unread_only: bool) -> Vec<usize> { self.core.filtered(query,unread_only) }
    fn older(&mut self) -> usize { self.core.load_older() }
    fn save_draft(&mut self, body: &str) { self.core.active_mut().draft = body.to_owned(); }
    fn take_thread_draft(&mut self, parent: usize) -> String {
        self.core.active_mut().thread_drafts.remove(&parent).unwrap_or_default()
    }
    fn save_thread_draft(&mut self, parent: usize, body: &str) {
        if !body.is_empty() { self.core.active_mut().thread_drafts.insert(parent,body.to_owned()); }
    }
    fn toggle_muted(&mut self) { let chat=self.core.active_mut(); chat.muted=!chat.muted; }
    fn send(&mut self) -> bool {
        let body = self.active().draft.clone();
        if !self.core.send() { return false; }
        self.record(Event::Send {chat:self.selected(),body});
        true
    }
    fn reply(&mut self, parent: usize, body: &str) -> bool {
        if !self.core.reply(parent,body) { return false; }
        self.record(Event::Reply {chat:self.selected(),parent,body:body.to_owned()});
        true
    }
    fn storage_status(&self) -> &'static str {
        if self.storage_error {"Не удалось сохранить на диск · только этот сеанс"}
        else if self.path.is_some() {"Отправленное сохраняется на этом компьютере"}
        else {"Только этот сеанс"}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn meeting_posts_and_replies_survive_restart() {
        let path=std::env::temp_dir().join(format!("bt-meeting-test-{}.json",std::process::id()));
        let _=fs::remove_file(&path);
        let mut s=LocalChatService::open(Some(path.clone()));
        s.select(8);
        assert_eq!(s.active().kind, crate::ChatKind::Meeting);
        assert!(s.reply(9997,"Follow-up"));
        s.save_draft("Meeting notes");
        assert!(s.send());
        assert!(s.reply(10000,"Attached context"));
        let mut s=LocalChatService::open(Some(path.clone()));
        s.select(8);
        assert_eq!(s.active().threads[&9997].last().unwrap().body,"Follow-up");
        assert_eq!(s.active().messages[60].body,"Meeting notes");
        assert_eq!(s.active().threads[&10000][0].body,"Attached context");
        fs::remove_file(path).unwrap();
    }
}
