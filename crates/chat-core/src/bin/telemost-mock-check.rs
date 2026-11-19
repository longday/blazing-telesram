use bt_chat_core::telemost_mock::{TelemostMock,fixture_call};
fn main() {
    let mut mock = TelemostMock::new(fixture_call);
    let chat = mock.chats().expect("list chats").remove(0);
    let messages = mock.history(&chat.id).expect("list history");
    let root = messages.iter().find(|m| m.reply_count>0).expect("thread root");
    let replies = mock.thread(&chat.id, &root.id).expect("list thread");
    println!("OFFLINE MOCK: {} · {} сообщений · {} ответа в ветке", chat.title,messages.len(),replies.len());
    println!("Синтетическая схема; запросов в Яндекс не было.");
}
