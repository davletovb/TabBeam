//! Provider-neutral prior messages for adapters without a usable native session.

use serde::Deserialize;

/// Only actual dialogue is sent to a provider; system instructions and page
/// context are never reconstructed from saved conversation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryMessage {
    pub role: Role,
    pub text: String,
}

/// Encodes previous messages as quoted JSON lines when a native continuation
/// cannot be recovered. The current question remains the final user message.
pub fn normalized_prompt(history: &[HistoryMessage], question: &str) -> String {
    let mut prompt = String::from(
        "Continue this conversation. The previous messages are quoted JSON data in order:\n",
    );
    for message in history {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let line = serde_json::json!({ "role": role, "text": message.text });
        prompt.push_str(&line.to_string());
        prompt.push('\n');
    }
    prompt.push_str("Current user question:\n");
    prompt.push_str(question);
    prompt
}
