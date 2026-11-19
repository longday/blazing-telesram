mod input;
use bt_chat_core::{ChatKind, ChatService, LocalChatService};
use gpui::{prelude::*, *};
use input::TextInput;

const BG: u32 = 0x111c2a;
const PANEL: u32 = 0x182536;
const ACCENT: u32 = 0xecab75;
const INK: u32 = 0xe6edf5;
const MUTED: u32 = 0x8fa3bb;
actions!(bt, [Send, ThreadSend, Search, Quit]);

struct Desktop {
    core: Box<dyn ChatService>, lists: Vec<ListState>, search: Entity<TextInput>, composer: Entity<TextInput>,
    folder: Option<ChatKind>, thread: Option<usize>, thread_input: Entity<TextInput>,
    unread_only: bool, show_info: bool, status: String,
}
impl Desktop {
    fn new(cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("Поиск чатов…",cx));
        let composer = cx.new(|cx| TextInput::new("Написать сообщение…",cx));
        cx.observe(&search, |_,_,cx| cx.notify()).detach();
        cx.observe(&composer, |_,_,cx| cx.notify()).detach();
        let thread_input = cx.new(|cx| TextInput::new("Ответить в ветке…",cx));
        let core: Box<dyn ChatService>=Box::new(LocalChatService::open(std::env::var_os("BT_STATE_PATH").map(Into::into)));
        let lists=core.chats().iter().map(|c| ListState::new(c.messages.len(),ListAlignment::Bottom,px(400.))).collect();
        Self { core, lists, search, composer, folder: None, thread:None, thread_input, unread_only:false, show_info:false, status:"Локальный режим · вымышленные данные".into() }
    }
    fn select(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.core.save_draft(&self.composer.read(cx).content);
        self.close_thread(cx);
        self.core.select(i); let draft=self.core.active().draft.clone();
        self.composer.update(cx, |input,cx|input.set(draft,cx));
        if self.core.active().kind!=ChatKind::Channel { window.focus(&self.composer.focus_handle(cx)); } cx.notify();
    }
    fn send(&mut self, _: &Send, _: &mut Window, cx: &mut Context<Self>) {
        self.core.save_draft(&self.composer.read(cx).content);
        let old=self.core.active().messages.len();
        if self.core.send() {
            self.lists[self.core.selected()].splice(old..old,2);
            self.lists[self.core.selected()].scroll_to(ListOffset { item_ix:old+2,offset_in_item:px(0.) });
            self.composer.update(cx,|i,cx|i.set(String::new(),cx));
            self.status="Сообщение и ответ добавлены локально".into();
        } else { self.status="Введите от 1 до 4000 символов".into(); }
        cx.notify();
    }
    fn older(&mut self,cx:&mut Context<Self>) {
        let state=&self.lists[self.core.selected()]; let anchor=state.logical_scroll_top();
        let n=self.core.older(); state.splice(0..0,n);
        state.scroll_to(ListOffset {item_ix:anchor.item_ix+n,offset_in_item:anchor.offset_in_item});
        self.status=if n>0 {format!("Загружено {n} сообщений · позиция сохранена")} else {"Начало истории".into()}; cx.notify();
    }
    fn close_thread(&mut self,cx:&mut Context<Self>) {
        if let Some(id)=self.thread.take() {
            let draft=self.thread_input.read(cx).content.to_string();
            self.core.save_thread_draft(id,&draft);
        }
    }
    fn open_thread(&mut self,id:usize,window:&mut Window,cx:&mut Context<Self>) {
        self.close_thread(cx);
        self.thread=Some(id);
        let draft=self.core.take_thread_draft(id);
        self.thread_input.update(cx,|i,cx|i.set(draft,cx));
        window.focus(&self.thread_input.focus_handle(cx));
        cx.notify();
    }
    fn reply(&mut self,_:&ThreadSend,_:&mut Window,cx:&mut Context<Self>) {
        if let Some(id)=self.thread {
            let draft=self.thread_input.read(cx).content.to_string();
            if self.core.reply(id,&draft) {
                self.thread_input.update(cx,|i,cx|i.set(String::new(),cx));
                cx.notify();
            }
        }
    }
    fn message(&self, ix:usize,cx:&mut Context<Self>) -> AnyElement {
        let m=&self.core.active().messages[ix];
        let outgoing=m.outgoing;
        let id=m.id;
        let kind=self.core.active().kind;
        let count=self.core.active().threads.get(&id).map_or(0,Vec::len);
        let label=match (kind,count) { (ChatKind::Channel,0)=>"Комментировать".into(), (ChatKind::Channel,n)=>format!("Комментарии · {n}"), (_,0)=>"Ответить в ветке".into(), (_,n)=>format!("Ответы · {n}") };
        div().w_full().px_5().py_2().flex().when(outgoing,|d|d.justify_end())
            .child(div().max_w(px(560.)).min_w_0().px_4().py_3().rounded_lg()
                .bg(rgb(if outgoing {0x304b5c} else {0x223146}))
                .child(div().text_size(px(15.)).line_height(px(22.)).child(m.body.clone()))
                .child(div().mt_2().text_xs().text_color(rgb(if outgoing {0xaacacb} else {MUTED}))
                    .child(format!("{}  ·  {:02}:{:02}{}",if outgoing {"Вы"} else {self.core.active().initials},9+(m.id/60)%12,m.id%60,if outgoing {"  ✓✓"} else {""})))
                .when(kind!=ChatKind::Personal,|d|d.child(div().id(("msg-thread",id)).mt_2().text_sm().py_1().cursor_pointer().text_color(rgb(if count>0 {ACCENT} else {MUTED}))
                    .hover(|el|el.text_color(rgb(ACCENT))).child(label)
                    .on_click(cx.listener(move|this,_,window,cx|this.open_thread(id,window,cx))))))
            .into_any_element()
    }
    fn thread_panel(&self,cx:&mut Context<Self>)->Option<AnyElement> {
        let id=self.thread?;
        let chat=self.core.active();
        let parent=chat.messages.iter().find(|m|m.id==id)?;
        let replies=chat.threads.get(&id);
        let n=replies.map_or(0,Vec::len);
        Some(div().w(px(350.)).flex_none().h_full().flex().flex_col().bg(rgb(PANEL)).border_l_1().border_color(rgb(0x38495b))
            .child(div().h(px(82.)).flex_none().p_4().flex().justify_between().items_center()
                .child(div().child(if chat.kind==ChatKind::Channel {"Комментарии"} else {"Ветка обсуждения"}).child(div().text_xs().text_color(rgb(MUTED)).child(format!("Ответы · {n}"))))
                .child(pill("close-thread","×").on_click(cx.listener(|this,_,_,cx|{this.close_thread(cx);cx.notify();}))))
            .child(div().id("thread-history").flex_1().min_h_0().overflow_y_scroll().px_4().pb_3()
                .child(div().p_3().mb_3().rounded_lg().bg(rgb(0x30465b)).text_sm().child(parent.body.clone()))
                .children(replies.into_iter().flatten().map(|r|div().p_3().mb_2().rounded_lg().bg(rgb(if r.author=="Вы" {0x304b5c} else {0x223146}))
                    .child(div().text_xs().text_color(rgb(ACCENT)).child(r.author)).child(div().text_sm().pt_1().child(r.body.clone()))))
                .when(n==0,|v|v.child(div().text_sm().text_color(rgb(MUTED)).child("Пока нет ответов. Начните обсуждение."))))
            .child(div().p_3().flex_none().border_t_1().border_color(rgb(0x38495b))
                .child(div().p_2().rounded_lg().bg(rgb(0x202e40)).key_context("ThreadComposer").child(self.thread_input.clone()))
                .child(div().mt_2().flex().justify_end().child(pill("reply-send","Ответить →").bg(rgb(ACCENT)).text_color(rgb(BG))
                    .on_click(cx.listener(|this,_,window,cx|this.reply(&ThreadSend,window,cx))))))
            .into_any_element())
    }
}
fn pill(id:&'static str, label:impl Into<SharedString>) -> Stateful<Div> {
    div().id(id).px_3().py_2().rounded_lg().text_sm().cursor_pointer().bg(rgb(0x243448))
        .hover(|s|s.bg(rgb(0x354a60))).child(label.into())
}
impl Render for Desktop {
    fn render(&mut self, _window:&mut Window,cx:&mut Context<Self>) -> impl IntoElement {
        let selected=self.core.selected();
        let query=self.search.read(cx).content.to_string();
        let filtered=self.core.search(&query,self.unread_only).into_iter().filter(|&i|self.folder.is_none_or(|kind|kind==self.core.chats()[i].kind)).collect::<Vec<_>>();
        let unread:usize=self.core.chats().iter().map(|c|c.unread).sum();
        let chat=self.core.active();
        let name=chat.name; let subtitle=chat.subtitle; let muted=chat.muted; let oldest=chat.oldest;
        let count=chat.messages.len();
        let kind=chat.kind;
        let thread_panel=self.thread_panel(cx);
        let rows=filtered.iter().map(|&i| {
            let c=&self.core.chats()[i];
            let preview=c.messages.back().map(|m|m.body.lines().next().unwrap_or("").to_string()).unwrap_or_default();
            div().id(("chat",i)).w_full().px_3().py_3().flex().gap_3().cursor_pointer().rounded_lg()
                .bg(rgb(if selected==i {0x2b3d51} else {PANEL}))
                .hover(|s|s.bg(rgb(0x293a4f)))
                .on_click(cx.listener(move |this,_,window,cx|this.select(i,window,cx)))
                .child(div().size_12().flex_none().rounded_full().bg(rgb([0x826451,0x547677,0x76688b,0x536e8e][i%4])).flex().items_center().justify_center().text_lg().child(c.initials))
                .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                    .child(div().flex().justify_between().gap_2().child(div().font_weight(FontWeight::SEMIBOLD).child(c.name)).child(div().text_xs().text_color(rgb(MUTED)).child(c.kind.label())))
                    .child(div().flex().gap_2().items_center().child(div().flex_1().min_w_0().truncate().text_sm().text_color(rgb(MUTED)).child(preview))
                        .when(c.unread>0,|d|d.child(div().px_2().rounded_full().text_xs().bg(rgb(ACCENT)).text_color(rgb(BG)).child(c.unread.to_string())))))
        }).collect::<Vec<_>>();
        let weak=cx.entity().downgrade();
        let history=list(self.lists[selected].clone(),move |ix,_,cx|weak.upgrade().unwrap().update(cx,|this,cx|this.message(ix,cx))).size_full();
        div().key_context("BlazingTelesram").size_full().flex().bg(rgb(BG)).text_color(rgb(INK)).font_family("sans-serif")
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(Self::reply))
            .on_action(cx.listener(|this,_:&Search,window,cx|window.focus(&this.search.focus_handle(cx))))
            .child(div().w(px(320.)).flex_none().h_full().flex().flex_col().bg(rgb(PANEL)).border_r_1().border_color(rgb(0x2a384a))
                .child(div().px_5().pt_5().pb_3().flex().gap_3().items_center()
                    .child(div().size_10().flex_none().rounded_lg().bg(rgb(ACCENT)).text_color(rgb(BG)).font_weight(FontWeight::BOLD).flex().items_center().justify_center().child("БТ"))
                    .child(div().flex().flex_col().child(div().font_weight(FontWeight::SEMIBOLD).child("Blazing Telesram")).child(div().text_xs().text_color(rgb(MUTED)).child("Офлайн 0.1 · Яндекс не подключён"))))
                .child(div().mx_4().mb_3().p_1().rounded_lg().bg(rgb(0x202e40)).child(self.search.clone()))
                .child(div().px_3().pb_2().flex().flex_wrap().gap_1().children([
                    ("all","Все",None),("personal","Личные",Some(ChatKind::Personal)),("groups","Группы",Some(ChatKind::Group)),("channels","Каналы",Some(ChatKind::Channel)),("meetings","Встречи",Some(ChatKind::Meeting))
                ].into_iter().map(|(id,label,folder)|pill(id,label).px_2().when(self.folder==folder,|d|d.bg(rgb(0x475464)))
                    .on_click(cx.listener(move|this,_,_,cx|{this.folder=folder;cx.notify();})))))
                .child(div().px_3().pb_2().flex().child(pill("unread",format!("Непрочитанные · {unread}")).when(self.unread_only,|d|d.bg(rgb(0x475464)))
                    .on_click(cx.listener(|this,_,_,cx|{this.unread_only=!this.unread_only;cx.notify();}))))
                .child(div().id("sidebar").flex_1().min_h_0().overflow_y_scroll().px_2().children(rows)
                    .when(filtered.is_empty(),|d|d.child(div().p_4().text_color(rgb(MUTED)).child("Чаты не найдены"))))
                .child(div().p_4().text_xs().text_color(rgb(MUTED)).child(self.core.storage_status())))
            .child(div().flex_1().min_w_0().h_full().flex().flex_col()
                .child(div().h(px(82.)).flex_none().px_7().flex().items_center().justify_between().bg(rgb(PANEL))
                    .child(div().flex().flex_col().gap_1().child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(name)).child(div().text_sm().text_color(rgb(MUTED)).child(format!("{} · {}",kind.label(),subtitle))))
                    .child(div().flex().gap_2().child(pill("mute",if muted {"Звук выкл."} else {"Звук вкл."}).on_click(cx.listener(|this,_,_,cx|{this.core.toggle_muted();cx.notify();})))
                        .child(pill("info","О чате").on_click(cx.listener(|this,_,_,cx|{this.show_info=!this.show_info;cx.notify();})))))
                .when(self.show_info,|d|d.child(div().px_7().py_3().text_sm().bg(rgb(0x26384a)).child(format!("{name} · {count} сообщений загружено из 10 000. Все участники вымышлены. Звук — демонстрационный переключатель."))))
                .child(div().py_3().flex_none().flex().justify_center().gap_3()
                    .child(pill("older",if oldest>0 {"↑ Загрузить старые"} else {"Начало истории"}).on_click(cx.listener(|this,_,_,cx|this.older(cx))))
                    .child(pill("latest","К последним ↓").on_click(cx.listener(|this,_,_,cx|{let n=this.core.active().messages.len();this.lists[this.core.selected()].scroll_to(ListOffset {item_ix:n,offset_in_item:px(0.)});cx.notify();}))))
                .child(div().flex_1().min_h_0().child(history))
                .when(kind==ChatKind::Channel,|d|d.child(div().p_4().bg(rgb(PANEL)).text_sm().text_color(rgb(MUTED)).child("Канал только для чтения. Ответы доступны в комментариях.")))
                .when(kind!=ChatKind::Channel,|d|d.child(div().px_6().pt_3().pb_4().flex_none().bg(rgb(PANEL))
                    .child(div().flex().items_center().gap_3()
                        .child(div().flex_1().min_w_0().p_2().rounded_lg().bg(rgb(0x202e40)).key_context("Composer").child(self.composer.clone()))
                        .child(pill("send","Отправить →").bg(rgb(ACCENT)).text_color(rgb(BG)).on_click(cx.listener(|this,_,window,cx|this.send(&Send,window,cx)))))
                    .child(div().mt_2().text_xs().text_color(rgb(MUTED)).child(format!("{}   ·   Enter — отправить",self.status))))))
            .children(thread_panel)
    }
}
fn main() {
    if let Err(error) = yandex_proto::runtime::RuntimeDir::from_env() {
        eprintln!("{error}");
        std::process::exit(2);
    }
    Application::new().run(|cx:&mut App| {
        input::bind(cx);
        cx.bind_keys([KeyBinding::new("enter",Send,Some("Composer")),KeyBinding::new("enter",ThreadSend,Some("ThreadComposer")),KeyBinding::new("cmd-f",Search,Some("BlazingTelesram")),KeyBinding::new("ctrl-f",Search,Some("BlazingTelesram")),KeyBinding::new("cmd-q",Quit,None)]);
        cx.on_action(|_:&Quit,cx|cx.quit());
        cx.on_window_closed(|cx| { if cx.windows().is_empty() {cx.quit();} }).detach();
        let bounds=Bounds::centered(None,size(px(1180.),px(800.)),cx);
        cx.open_window(WindowOptions {window_bounds:Some(WindowBounds::Windowed(bounds)),window_min_size:Some(size(px(850.),px(560.))),titlebar:Some(TitlebarOptions {title:Some("Blazing Telesram".into()),..Default::default()}),..Default::default()},|_,cx|cx.new(Desktop::new)).expect("Unable to open native window; check display/GPU availability");
        cx.activate(true);
    });
}
