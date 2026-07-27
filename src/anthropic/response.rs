use std::collections::HashMap;

use serde_json::Value;

use crate::kiro::model::events::Event;

enum ContentBlock {
    Thinking {
        text: String,
        signature: Option<String>,
    },
    RedactedThinking(String),
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: String,
        complete: bool,
    },
}

pub(crate) struct AggregatedContent {
    pub content: Vec<Value>,
    pub has_tool_use: bool,
    pub has_invalid_reasoning: bool,
}

pub(crate) fn aggregate_content(
    events: &[Event],
    thinking_enabled: bool,
    tool_name_map: &HashMap<String, String>,
) -> AggregatedContent {
    let mut blocks = Vec::new();

    for event in events {
        match event {
            Event::ReasoningContent(reasoning) => {
                if let Some(data) = reasoning
                    .redacted_content
                    .as_ref()
                    .filter(|value| !value.is_empty())
                {
                    blocks.push(ContentBlock::RedactedThinking(data.clone()));
                } else if !reasoning.text.is_empty() || reasoning.signature.is_some() {
                    match blocks.last_mut() {
                        Some(ContentBlock::Thinking { text, signature }) => {
                            text.push_str(&reasoning.text);
                            if reasoning.signature.is_some() {
                                *signature = reasoning.signature.clone();
                            }
                        }
                        _ => blocks.push(ContentBlock::Thinking {
                            text: reasoning.text.clone(),
                            signature: reasoning.signature.clone(),
                        }),
                    }
                }
            }
            Event::AssistantResponse(response) if !response.content.is_empty() => {
                match blocks.last_mut() {
                    Some(ContentBlock::Text(text)) => text.push_str(&response.content),
                    _ => blocks.push(ContentBlock::Text(response.content.clone())),
                }
            }
            Event::ToolUse(tool_use) => {
                if let Some(ContentBlock::ToolUse {
                    input, complete, ..
                }) = blocks.iter_mut().find(|block| {
                    matches!(block, ContentBlock::ToolUse { id, .. } if id == &tool_use.tool_use_id)
                }) {
                    input.push_str(&tool_use.input);
                    *complete |= tool_use.stop;
                } else {
                    blocks.push(ContentBlock::ToolUse {
                        id: tool_use.tool_use_id.clone(),
                        name: tool_use.name.clone(),
                        input: tool_use.input.clone(),
                        complete: tool_use.stop,
                    });
                }
            }
            Event::AssistantResponse(_)
            | Event::Metering(_)
            | Event::ContextUsage(_)
            | Event::Unknown {}
            | Event::Error { .. }
            | Event::Exception { .. } => {}
        }
    }

    let has_native_reasoning = blocks.iter().any(|block| {
        matches!(
            block,
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking(_)
        )
    });
    let has_tool_use = blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { complete: true, .. }));
    let has_invalid_reasoning = thinking_enabled
        && blocks.iter().any(|block| {
            matches!(
                block,
                ContentBlock::Thinking { signature, .. }
                    if signature
                        .as_deref()
                        .is_none_or(|value| value.trim().is_empty())
            )
        });
    let mut content = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Thinking {
                text,
                signature: Some(signature),
            } if thinking_enabled && !signature.trim().is_empty() => {
                content.push(serde_json::json!({
                    "type": "thinking",
                    "thinking": text,
                    "signature": signature
                }));
            }
            ContentBlock::RedactedThinking(data) if thinking_enabled => {
                content.push(serde_json::json!({
                    "type": "redacted_thinking",
                    "data": data
                }));
            }
            ContentBlock::Text(text) if thinking_enabled && !has_native_reasoning => {
                let (thinking, remaining) =
                    super::stream::extract_thinking_from_complete_text(&text);
                if let Some(thinking) = thinking {
                    content.push(serde_json::json!({
                        "type": "thinking",
                        "thinking": thinking
                    }));
                }
                if !remaining.is_empty() {
                    content.push(serde_json::json!({"type": "text", "text": remaining}));
                }
            }
            ContentBlock::Text(text) if !text.is_empty() => {
                content.push(serde_json::json!({"type": "text", "text": text}));
            }
            ContentBlock::ToolUse {
                id,
                name,
                input,
                complete: true,
            } => {
                let input = if input.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&input).unwrap_or_else(|error| {
                        tracing::warn!(tool_use_id = %id, %error, "工具输入 JSON 解析失败");
                        serde_json::json!({})
                    })
                };
                let name = tool_name_map.get(&name).cloned().unwrap_or(name);
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input
                }));
            }
            ContentBlock::Thinking { .. }
            | ContentBlock::RedactedThinking(_)
            | ContentBlock::Text(_)
            | ContentBlock::ToolUse {
                complete: false, ..
            } => {}
        }
    }

    AggregatedContent {
        content,
        has_tool_use,
        has_invalid_reasoning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::model::events::{AssistantResponseEvent, ReasoningContentEvent, ToolUseEvent};

    #[test]
    fn test_aggregate_content_preserves_native_block_order() {
        // Given: interleaved reasoning, text, tool, text, and redacted events.
        let mut before = AssistantResponseEvent::default();
        before.content = "before".to_string();
        let mut after = AssistantResponseEvent::default();
        after.content = "after".to_string();
        let events = vec![
            Event::ReasoningContent(ReasoningContentEvent {
                text: "thought".to_string(),
                signature: Some("sig".to_string()),
                redacted_content: None,
            }),
            Event::AssistantResponse(before),
            Event::ToolUse(ToolUseEvent {
                name: "read_file".to_string(),
                tool_use_id: "toolu_1".to_string(),
                input: "{}".to_string(),
                stop: true,
            }),
            Event::AssistantResponse(after),
            Event::ReasoningContent(ReasoningContentEvent {
                text: String::new(),
                signature: None,
                redacted_content: Some("opaque".to_string()),
            }),
        ];

        // When: the non-stream response content is aggregated.
        let result = aggregate_content(&events, true, &HashMap::new());

        // Then: Anthropic content blocks retain the upstream event order.
        let block_types: Vec<_> = result
            .content
            .iter()
            .filter_map(|block| block["type"].as_str())
            .collect();
        assert_eq!(
            block_types,
            ["thinking", "text", "tool_use", "text", "redacted_thinking"]
        );
        assert!(result.has_tool_use);
    }

    #[test]
    fn test_aggregate_content_preserves_signature_only_thinking() {
        // Given: display=omitted reasoning with only a signature event.
        let events = vec![Event::ReasoningContent(ReasoningContentEvent {
            text: String::new(),
            signature: Some("sig-omitted".to_string()),
            redacted_content: None,
        })];

        // When: the non-stream response content is aggregated.
        let result = aggregate_content(&events, true, &HashMap::new());

        // Then: an empty thinking block retains its required signature.
        assert_eq!(
            result.content,
            [serde_json::json!({
                "type": "thinking",
                "thinking": "",
                "signature": "sig-omitted"
            })]
        );
    }

    #[test]
    fn test_aggregate_content_omits_incomplete_tool_use() {
        // Given: a partial Kiro tool event without its stop marker.
        let events = vec![Event::ToolUse(ToolUseEvent {
            name: "read_file".to_string(),
            tool_use_id: "toolu_partial".to_string(),
            input: r#"{"path":"README""#.to_string(),
            stop: false,
        })];

        // When: the non-stream response content is aggregated.
        let result = aggregate_content(&events, false, &HashMap::new());

        // Then: no malformed partial tool block is exposed to the client.
        assert!(result.content.is_empty());
        assert!(!result.has_tool_use);
    }

    #[test]
    fn test_aggregate_content_marks_native_reasoning_without_signature_invalid() {
        // Given: native reasoning text without the signature required for replay.
        let events = vec![Event::ReasoningContent(ReasoningContentEvent {
            text: "thought".to_string(),
            signature: None,
            redacted_content: None,
        })];

        // When: the non-stream response is aggregated.
        let result = aggregate_content(&events, true, &HashMap::new());

        // Then: the handler can reject the malformed native block without emitting it.
        assert!(result.has_invalid_reasoning);
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_aggregate_content_marks_whitespace_reasoning_signature_invalid() {
        let events = vec![Event::ReasoningContent(ReasoningContentEvent {
            text: "thought".to_string(),
            signature: Some("   ".to_string()),
            redacted_content: None,
        })];

        let result = aggregate_content(&events, true, &HashMap::new());

        assert!(result.has_invalid_reasoning);
        assert!(result.content.is_empty());
    }
}
