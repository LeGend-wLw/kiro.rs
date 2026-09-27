use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

use super::types::{Message, MessagesRequest};

pub struct TruncationState {
    inner: Mutex<TruncationEntries>,
}

struct TruncationEntries {
    tools: HashMap<String, String>,
    content: HashMap<u64, ()>,
}

impl TruncationState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(TruncationEntries {
                tools: HashMap::new(),
                content: HashMap::new(),
            }),
        })
    }

    pub fn save_tool(&self, tool_use_id: &str, tool_name: &str) {
        let mut entries = lock_entries(&self.inner);
        entries
            .tools
            .insert(tool_use_id.to_string(), tool_name.to_string());
    }

    fn take_tool(&self, tool_use_id: &str) -> Option<String> {
        lock_entries(&self.inner).tools.remove(tool_use_id)
    }

    pub fn save_content(&self, content: &str) {
        lock_entries(&self.inner)
            .content
            .insert(content_hash(content), ());
    }

    fn take_content(&self, content: &str) -> bool {
        lock_entries(&self.inner)
            .content
            .remove(&content_hash(content))
            .is_some()
    }
}

pub fn record_truncated_response(
    state: &TruncationState,
    content: &str,
    tool_calls: &HashMap<String, (String, bool)>,
) {
    let incomplete_tools = tool_calls.iter().filter(|(_, (_, complete))| !complete);
    let mut found_incomplete_tool = false;
    for (tool_use_id, (tool_name, _)) in incomplete_tools {
        state.save_tool(tool_use_id, tool_name);
        found_incomplete_tool = true;
    }
    if !found_incomplete_tool && !content.is_empty() {
        state.save_content(content);
    }
}

pub fn apply_recovery(request: &mut MessagesRequest, state: &TruncationState) {
    let mut recovered = Vec::with_capacity(request.messages.len());
    for message in request.messages.drain(..) {
        if message.role == "user" {
            if let Some(content) = message.content.as_array() {
                let mut blocks = content.clone();
                let mut modified = false;
                for block in &mut blocks {
                    if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_result")
                    {
                        continue;
                    }
                    let Some(tool_use_id) =
                        block.get("tool_use_id").and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    if state.take_tool(tool_use_id).is_some() {
                        let original = block
                            .get("content")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!(""));
                        let notice = "[API Limitation] The previous tool call was truncated by Kiro due to output size limits. Repeating the exact same operation may be truncated again; adapt the operation before retrying.";
                        block["content"] = serde_json::json!(format!(
                            "{notice}\n\nOriginal tool result:\n{original}"
                        ));
                        block["is_error"] = serde_json::json!(true);
                        modified = true;
                    }
                }
                if modified {
                    recovered.push(Message {
                        role: message.role,
                        content: serde_json::Value::Array(blocks),
                    });
                    continue;
                }
            }
        }

        if message.role == "assistant" {
            if let Some(content) = assistant_text(&message.content) {
                let is_truncated = state.take_content(&content);
                recovered.push(message);
                if is_truncated {
                    recovered.push(Message {
                        role: "user".to_string(),
                        content: serde_json::json!("[System Notice] The previous response was truncated by Kiro due to output size limits. Continue with a smaller operation instead of repeating the same output."),
                    });
                }
                continue;
            }
        }

        recovered.push(message);
    }
    request.messages = recovered;
}

fn assistant_text(content: &serde_json::Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let blocks = content.as_array()?;
    let text = blocks
        .iter()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

fn content_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content
        .chars()
        .take(500)
        .collect::<String>()
        .hash(&mut hasher);
    hasher.finish()
}

fn lock_entries(mutex: &Mutex<TruncationEntries>) -> MutexGuard<'_, TruncationEntries> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_assistant_message_gets_recovery_notice_on_next_request() {
        let state = TruncationState::new();
        let mut request = MessagesRequest {
            model: "claude-sonnet-4.5".to_string(),
            max_tokens: 100,
            messages: vec![Message {
                role: "assistant".to_string(),
                content: serde_json::json!("partial output"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        };

        state.save_content("partial output");
        apply_recovery(&mut request, &state);

        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[1].role, "user");
        assert!(
            request.messages[1]
                .content
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }

    #[test]
    fn truncated_tool_result_gets_recovery_notice_on_next_request() {
        let state = TruncationState::new();
        let mut request = MessagesRequest {
            model: "claude-sonnet-4.5".to_string(),
            max_tokens: 100,
            messages: vec![Message {
                role: "user".to_string(),
                content: serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": "partial file output"
                }]),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        };

        state.save_tool("toolu_1", "Write");
        apply_recovery(&mut request, &state);

        assert_eq!(request.messages[0].content[0]["is_error"], true);
        assert!(
            request.messages[0]
                .content
                .to_string()
                .contains("partial file output")
        );
    }
}
