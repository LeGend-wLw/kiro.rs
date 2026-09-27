//! 推理内容事件
//!
//! 处理 reasoningContentEvent — Q Developer 在 thinking 模式下推送的服务端
//! 推理内容流。对应 Anthropic 的 `thinking` content_block + thinking_delta /
//! signature_delta SSE 事件。
//!
//! payload 形态（实测）：
//!   `{ "text": "..." }`                     — 增量文本
//!   `{ "text": "...", "signature": "..." }` — 末尾的服务端签名（多轮 cache 必需）
//!   `{ "redactedContent": "..." }`          — 服务端打码的不可读内容（可丢）

use serde::{Deserialize, Serialize};

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

/// 推理内容事件负载
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningContentEvent {
    /// 增量推理文本
    #[serde(default)]
    pub text: String,
    /// 服务端签名（仅在 reasoning 块结束时出现，客户端必须在下一轮原样回放以维持 cache）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// 打码内容（不可读，可忽略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_content: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReasoningFields {
    #[serde(default, alias = "thinkingText", alias = "content")]
    text: String,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    redacted_content: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ReasoningWire {
    Wrapped {
        #[serde(rename = "reasoningContentEvent")]
        event: Box<ReasoningWire>,
    },
    Union {
        #[serde(rename = "reasoningText")]
        reasoning_text: ReasoningFields,
    },
    Flat(ReasoningFields),
}

impl From<ReasoningWire> for ReasoningContentEvent {
    fn from(wire: ReasoningWire) -> Self {
        let fields = match wire {
            ReasoningWire::Wrapped { event } => return Self::from(*event),
            ReasoningWire::Union { reasoning_text } => reasoning_text,
            ReasoningWire::Flat(fields) => fields,
        };
        Self {
            text: fields.text,
            signature: fields.signature,
            redacted_content: fields.redacted_content,
        }
    }
}

impl<'de> Deserialize<'de> for ReasoningContentEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ReasoningWire::deserialize(deserializer).map(Self::from)
    }
}

impl EventPayload for ReasoningContentEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_text_only() {
        let json = r#"{"text":"用户需要..."}"#;
        let ev: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.text, "用户需要...");
        assert!(ev.signature.is_none());
    }

    #[test]
    fn test_deserialize_with_signature() {
        let json = r#"{"text":"final","signature":"sig-abc"}"#;
        let ev: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.signature.as_deref(), Some("sig-abc"));
    }

    #[test]
    fn test_deserialize_redacted() {
        let json = r#"{"redactedContent":"opaque-blob"}"#;
        let ev: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.redacted_content.as_deref(), Some("opaque-blob"));
        assert_eq!(ev.text, "");
    }

    #[test]
    fn test_deserialize_reasoning_text_union() {
        // Given: the newer reasoning union shape used by some Kiro responses.
        let json = r#"{"reasoningText":{"text":"step","signature":"sig-union"}}"#;

        // When: the payload crosses the event boundary.
        let event: ReasoningContentEvent = serde_json::from_str(json).unwrap();

        // Then: it has the same normalized fields as the flat event shape.
        assert_eq!(event.text, "step");
        assert_eq!(event.signature.as_deref(), Some("sig-union"));
    }

    #[test]
    fn test_deserialize_wrapped_reasoning_event() {
        // Given: an event payload wrapped by its event name.
        let json = r#"{"reasoningContentEvent":{"text":"wrapped","signature":"sig-wrapped"}}"#;

        // When: the payload crosses the event boundary.
        let event: ReasoningContentEvent = serde_json::from_str(json).unwrap();

        // Then: the wrapper is removed without losing signed reasoning.
        assert_eq!(event.text, "wrapped");
        assert_eq!(event.signature.as_deref(), Some("sig-wrapped"));
    }
}
