//! Pure, deterministic, offline chat model. No I/O or UI dependencies.
use std::collections::VecDeque;
mod service;
pub mod telemost_mock;
pub use service::{ChatService, LocalChatService};
pub const TOTAL: usize = 10_000;
pub const PAGE: usize = 60;
pub const MAX_DRAFT: usize = 4000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message { pub id: usize, pub body: String, pub outgoing: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatKind { Personal, Group, Channel, Meeting }
impl ChatKind { pub fn label(self) -> &'static str { match self {Self::Meeting=>"Встреча",Self::Personal=>"Личный",Self::Group=>"Группа",Self::Channel=>"Канал"} } }
#[derive(Debug)]
pub struct Reply { pub author: &'static str, pub body: String }
#[derive(Debug)]
pub struct Chat {
    pub kind: ChatKind,
    pub threads: std::collections::HashMap<usize, Vec<Reply>>,
    pub thread_drafts: std::collections::HashMap<usize, String>,
    pub name: &'static str, pub initials: &'static str, pub subtitle: &'static str,
    pub unread: usize, pub muted: bool, pub draft: String,
    pub messages: VecDeque<Message>, pub oldest: usize, next_id: usize,
}
#[derive(Debug)]
pub struct Core { pub chats: Vec<Chat>, pub selected: usize }
impl Default for Core { fn default() -> Self { Self::new() } }
impl Core {
    pub fn new() -> Self {
        let names = [("Студия Север", "СС", "Команда · 8 участников", 4),
            ("Аня Смирнова", "АС", "Дизайнер интерфейсов", 2),
            ("Поход в субботу", "ПС", "Группа · 12 участников", 18),
            ("Марк", "М", "Личный чат", 0),
            ("Новости продукта", "НП", "4 281 подписчик", 0),
            ("Книжный клуб", "КК", "Группа · 24 участника", 7),
            ("Лера", "Л", "Личный чат", 0),
            ("Музыка на вечер", "МВ", "1 240 подписчиков", 0), ("Дизайн-ревью БТ", "ДР", "15:30–16:00 · 8 участников · встреча завершена", 0)];
        let chats = names.into_iter().enumerate().map(|(seed,(name,initials,subtitle,unread))| Chat {
            name, initials, subtitle, unread, kind: match seed { 8=>ChatKind::Meeting, 0|2|5=>ChatKind::Group, 4|7=>ChatKind::Channel, _=>ChatKind::Personal },
            threads: if matches!(seed,0|2|4|5|7|8) { [(TOTAL-3, vec![Reply { author:"Ника",body:"Посмотрела, спасибо! Давайте оставим подробности здесь.".into() }, Reply { author:"Денис", body:"Согласен. Я добавлю заметки позже.".into() }])].into() } else { Default::default() },
            thread_drafts: Default::default(),
            muted: false, draft: String::new(),
            messages: (TOTAL-PAGE..TOTAL).map(|id| synthetic(seed,id)).collect(),
            oldest: TOTAL-PAGE, next_id: TOTAL,
        }).collect();
        let mut core = Self { chats, selected: 0 }; core.select(0); core
    }
    pub fn select(&mut self, index: usize) { if index < self.chats.len() { self.selected = index; self.chats[index].unread = 0; } }
    pub fn active(&self) -> &Chat { &self.chats[self.selected] }
    pub fn active_mut(&mut self) -> &mut Chat { &mut self.chats[self.selected] }
    pub fn filtered(&self, query: &str, unread_only: bool) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        self.chats.iter().enumerate().filter(|(_,c)| (!unread_only || c.unread > 0) && c.name.to_lowercase().contains(&query)).map(|(i,_)| i).collect()
    }
    /// Returns exact prepend count; UI keeps its logical item anchor plus this count.
    pub fn load_older(&mut self) -> usize {
        let seed = self.selected; let chat = self.active_mut();
        let start = chat.oldest.saturating_sub(PAGE); let count = chat.oldest-start;
        for id in (start..chat.oldest).rev() { chat.messages.push_front(synthetic(seed,id)); }
        chat.oldest = start; count
    }
    pub fn reply(&mut self, parent:usize, text:&str)->bool {
        let chat=self.active_mut();
        let text=text.trim();
        if text.is_empty() || text.chars().count()>MAX_DRAFT || !chat.messages.iter().any(|m|m.id==parent) {return false}
        chat.threads.entry(parent).or_default().push(Reply{author:"Вы",body:text.into()});
        chat.thread_drafts.remove(&parent);
        true
    }
    /// Atomically adds the local message and a deterministic simulated reply.
    pub fn send(&mut self) -> bool {
        let chat = self.active_mut(); if chat.kind==ChatKind::Channel {return false} let text = chat.draft.trim();
        if text.is_empty() || text.chars().count() > MAX_DRAFT { return false; }
        let body = text.to_owned(); let reply = format!("Получено: «{}». Это локальный ответ БТ — сообщение никуда не отправлялось.", body.chars().take(90).collect::<String>());
        chat.messages.push_back(Message { id: chat.next_id, body, outgoing: true });
        chat.messages.push_back(Message { id: chat.next_id+1, body: reply, outgoing: false });
        chat.next_id += 2; chat.draft.clear(); true
    }
}
pub fn synthetic(seed: usize, id: usize) -> Message {
    let samples = ["Привет! Как проходит день?", "Собрали первые идеи. Мне нравится спокойная палитра и больше воздуха между блоками.",
        "Да, согласен. Давайте посмотрим вместе.", "План на сегодня:\n• обсудить детали\n• собрать рабочий прототип\n• проверить длинную историю\n\nГлавное — чтобы интерфейс оставался отзывчивым.",
        "Отлично, беру в работу!", "Небольшое обновление: всё готово к обсуждению. Можно переключаться между чатами, искать по имени и писать сообщения.\n\nВсе данные в этой версии вымышлены.",
        "Встретимся в 16:30?", "Звучит хорошо. До встречи!"];
    let body=match (seed,id) {
        (8,9996)=>"Начинаем дизайн-ревью. Повестка: навигация, поиск и ширина панели обсуждений.",
        (8,9997)=>"Итоги встречи: оставляем папки в боковой панели. Замечания к прототипу разберём в этой ветке.",
        (8,9998)=>"Я пересмотрю ширину панели обсуждений и покажу два варианта на следующей встрече.",
        (8,9999)=>"Встреча завершена. Чат остаётся открытым для продолжения обсуждения.",
        (0,9997)=>"Макеты главной готовы. Давайте соберём замечания в ветке до пятницы.",
        (2,9997)=>"Маршрут на субботу: встречаемся у входа в парк в 9:30. Кто сможет принести аптечку?",
        (4,9997)=>"Обновление 0.2 · В приложении появились папки и ветки обсуждений. Делитесь отзывами в комментариях к этому посту.",
        (7,9997)=>"Пятничная подборка · Пять альбомов для спокойного вечера. Свои находки оставляйте в комментариях.",
        (5,9997)=>"Следующая встреча — обсуждаем «Солярис». Что стоит перечитать перед встречей?",
        (4|7,_)=>["Дайджест недели: небольшая подборка новостей и полезных материалов.", "Новый выпуск уже доступен. Подробности обсуждаем в комментариях.", "Небольшой анонс: следующий выпуск выйдет в пятницу."][id%3],
        _=>samples[(id+seed*3)%samples.len()]
    };
    Message { id, body:body.into(), outgoing: !matches!(seed,4|7) && id%4 == 2 }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn pages_are_contiguous_deterministic_and_exhaustible() {
        let mut a = Core::new(); let mut b = Core::new();
        while a.active().oldest > 0 { assert_eq!(a.load_older(), b.load_older()); }
        assert_eq!(a.load_older(), 0); assert_eq!(a.active().messages.len(), TOTAL);
        assert_eq!(a.active().messages, b.active().messages);
        assert!(a.active().messages.iter().enumerate().all(|(i,m)| i == m.id));
    }
    #[test] fn prepend_keeps_message_and_intra_item_anchor() {
        let mut c=Core::new(); let index=23; let pixel_offset=17.5;
        let id=c.active().messages[index].id; let n=c.load_older();
        assert_eq!(c.active().messages[index+n].id,id); assert_eq!(pixel_offset,17.5);
    }
    #[test] fn send_is_local_ordered_and_preserves_other_drafts() {
        let mut c=Core::new(); c.active_mut().draft="  Привет 🦀  ".into();
        assert!(c.send()); assert_eq!(c.active().messages[PAGE].body,"Привет 🦀");
        assert_eq!(c.active().messages[PAGE+1].id,TOTAL+1); assert!(c.active().draft.is_empty());
        assert!(!c.send()); c.active_mut().draft="черновик".into(); c.select(1); c.select(0);
        assert_eq!(c.active().draft,"черновик");
    }
    #[test] fn isolated_thread_reply_and_channel_read_only() {
        let mut c=Core::new();
        assert_eq!(c.active().threads[&9997].len(),2);
        let timeline_before = c.active().messages.clone();
        assert!(c.reply(9997," test "));
        assert_eq!(c.active().messages, timeline_before, "thread reply must not enter the main timeline");
        assert!(!c.reply(25,"unknown"));
        assert_eq!(c.active().threads[&9997][2].body,"test");
        c.select(4);
        assert_eq!(c.active().kind,ChatKind::Channel);
        assert_eq!(c.active().threads[&9997].len(),2);
        c.active_mut().draft="no publication".into();
        assert!(!c.send());
        assert!(c.reply(9997,"ok"));
    }
    #[test] fn search_and_read_state() {
        let mut c=Core::new(); assert_eq!(c.filtered(" АНЯ ",false),vec![1]);
        assert!(c.filtered("",true).contains(&1)); c.select(1);
        assert!(!c.filtered("",true).contains(&1)); c.select(999); assert_eq!(c.selected,1);
    }
    #[test] fn huge_or_blank_draft_is_rejected() {
        let mut c=Core::new(); c.active_mut().draft=" \n ".into(); assert!(!c.send());
        c.active_mut().draft="я".repeat(MAX_DRAFT+1); assert!(!c.send());
    }
    #[test] fn prepend_after_send_has_no_duplicate_ids() {
        let mut c=Core::new(); c.active_mut().draft="тест".into(); c.send(); c.load_older();
        let ids: std::collections::HashSet<_>=c.active().messages.iter().map(|m|m.id).collect();
        assert_eq!(ids.len(),c.active().messages.len());
    }
}
