//! Offline MCP contract slice. Tool names are documented; fixture argument/result
//! fields are local proposals, not assertions about the real site's wire format.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const CHATS: &str = "telemost.list_chats";
const HISTORY: &str = "telemost.list_chat_messages";
const THREAD: &str = "telemost.list_thread_messages";

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChatSummary { pub id: String, pub title: String }

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChatMessage { pub id: String, pub text: String, pub reply_count: usize }

#[derive(Debug, PartialEq, Eq)]
pub enum MockError { MissingFixture, InvalidEnvelope, ToolFailed, InvalidPayload }

/// This adapter accepts only an in-process function. No network transport is provided.
pub struct TelemostMock<F> { call: F, sequence: u64 }
impl<F: FnMut(Value) -> Result<Value, MockError>> TelemostMock<F> {
    pub fn new(call: F) -> Self { Self { call, sequence: 0 } }
    fn read<T: DeserializeOwned>(&mut self, name: &str, arguments: Value) -> Result<T, MockError> {
        self.sequence += 1;
        let id = self.sequence;
        let response = (self.call)(json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }))?;
        if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || response.get("id").and_then(Value::as_u64) != Some(id) { return Err(MockError::InvalidEnvelope); }
        if response.get("error").is_some() || response.pointer("/result/isError").and_then(Value::as_bool)==Some(true) {
            return Err(MockError::ToolFailed);
        }
        let data = response.pointer("/result/structuredContent").ok_or(MockError::InvalidEnvelope)?;
        serde_json::from_value(data.clone()).map_err(|_|MockError::InvalidPayload)
    }
    pub fn chats(&mut self) -> Result<Vec<ChatSummary>, MockError> {
        #[derive(Deserialize)] struct Chats { chats: Vec<ChatSummary> }
        Ok(self.read::<Chats>(CHATS, json!({}))?.chats)
    }
    pub fn history(&mut self, chat_id: &str) -> Result<Vec<ChatMessage>, MockError> {
        self.message_list(HISTORY, json!({ "chat_id": chat_id }))
    }
    pub fn thread(&mut self, chat_id: &str, root_id: &str) -> Result<Vec<ChatMessage>, MockError> {
        self.message_list(THREAD, json!({ "chat_id": chat_id, "root_id": root_id }))
    }
    fn message_list(&mut self, tool: &str, arguments: Value) -> Result<Vec<ChatMessage>, MockError> {
        #[derive(Deserialize)] struct Messages { messages: Vec<ChatMessage> }
        Ok(self.read::<Messages>(tool, arguments)?.messages)
    }
}

/// Synthetic in-process fixture server. Matches both tool name and supplied IDs.
pub fn fixture_call(request: Value) -> Result<Value, MockError> {
    if request["jsonrpc"] != "2.0" || request["method"] != "tools/call" || !request["id"].is_u64() {
        return Err(MockError::InvalidEnvelope);
    }
    let id = request["id"].clone();
    let fixtures: Value = serde_json::from_str(include_str!("../tests/fixtures/telemost-mcp.json"))
        .map_err(|_| MockError::InvalidPayload)?;
    let tool = request.pointer("/params/name").and_then(Value::as_str).ok_or(MockError::InvalidEnvelope)?;
    let record = fixtures.get(tool).ok_or(MockError::MissingFixture)?;
    if request.pointer("/params/arguments") != record.get("arguments") { return Ok(json!({"jsonrpc":"2.0","id":id,"result":{"isError":true}})); }
    Ok(json!({"jsonrpc":"2.0","id":id,"result":{"structuredContent":record["payload"]}}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn fixture_handles_chat_history_and_thread() {
        let mut mock = TelemostMock::new(fixture_call);
        let chats = mock.chats().unwrap();
        let messages = mock.history(&chats[0].id).unwrap();
        let root = messages.first().unwrap();
        assert!(root.reply_count > 0);
        assert_eq!(mock.thread(&chats[0].id, &root.id).unwrap().len(), root.reply_count);
    }
    #[test] fn unknown_chat_or_parent_is_a_tool_error() {
        let mut mock = TelemostMock::new(fixture_call);
        assert_eq!(mock.history("unknown"), Err(MockError::ToolFailed));
        assert_eq!(mock.thread("group-demo", "unknown"), Err(MockError::ToolFailed));
    }
    #[test] fn mismatched_response_id_is_rejected() {
        let mut mock = TelemostMock::new(|_| Ok(json!({"jsonrpc":"2.0","id":99})));
        assert_eq!(mock.chats(), Err(MockError::InvalidEnvelope));
    }
    #[test] fn invalid_payload_is_rejected() {
        let mut mock = TelemostMock::new(|request| Ok(json!({"jsonrpc":"2.0","id":request["id"],"result":{"structuredContent":{"chats":"unexpected"}}})));
        assert_eq!(mock.chats(), Err(MockError::InvalidPayload));
    }
    #[test] fn no_send_fixture_exists() {
        assert_eq!(fixture_call(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"telemost.send_message","arguments":{}}})),Err(MockError::MissingFixture));
    }
}
