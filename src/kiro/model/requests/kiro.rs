//! Kiro 请求类型定义
//!
//! 定义 Kiro API 的主请求结构

use serde::{Deserialize, Serialize};

use super::conversation::{ConversationState, Message};

/// Kiro API 请求
///
/// 用于构建发送给 Kiro API 的请求
///
/// # 示例
///
/// ```rust
/// use kiro_rs::kiro::model::requests::{
///     KiroRequest, ConversationState, CurrentMessage, UserInputMessage, Tool
/// };
///
/// // 创建简单请求
/// let state = ConversationState::new("conv-123")
///     .with_agent_task_type("vibe")
///     .with_current_message(CurrentMessage::new(
///         UserInputMessage::new("Hello", "claude-3-5-sonnet")
///     ));
///
/// let request = KiroRequest::new(state);
/// let json = request.to_json().unwrap();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroRequest {
    /// 对话状态
    pub conversation_state: ConversationState,
    /// Profile ARN（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,
    /// 顶层 additionalModelRequestFields（原生 reasoning effort 控制），与 conversationState 平级
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_model_request_fields: Option<serde_json::Value>,
}

impl KiroRequest {
    pub fn without_reasoning_content(&self) -> Option<Self> {
        let mut fallback = self.clone();
        let mut changed = false;

        for message in &mut fallback.conversation_state.history {
            if let Message::Assistant(assistant) = message {
                changed |= assistant
                    .assistant_response_message
                    .reasoning_content
                    .take()
                    .is_some();
            }
        }

        changed.then_some(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_kiro_request_deserialize() {
        let json = r#"{
            "conversationState": {
                "conversationId": "conv-456",
                "currentMessage": {
                    "userInputMessage": {
                        "content": "Test message",
                        "modelId": "claude-3-5-sonnet",
                        "userInputMessageContext": {}
                    }
                }
            }
        }"#;

        let request: KiroRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.conversation_state.conversation_id, "conv-456");
        assert_eq!(
            request
                .conversation_state
                .current_message
                .user_input_message
                .content,
            "Test message"
        );
    }

    #[test]
    fn test_without_reasoning_content_preserves_the_rest_of_history() {
        // Given: a request with signed reasoning and a tool use in assistant history.
        let request: KiroRequest = serde_json::from_value(serde_json::json!({
            "conversationState": {
                "conversationId": "conv-retry",
                "currentMessage": {
                    "userInputMessage": {
                        "content": "continue",
                        "modelId": "claude-opus-5",
                        "userInputMessageContext": {}
                    }
                },
                "history": [{
                    "assistantResponseMessage": {
                        "content": "answer",
                        "reasoningContent": {
                            "reasoningText": {"text": "thought", "signature": "sig"}
                        },
                        "toolUses": [{
                            "toolUseId": "toolu_1",
                            "name": "read_file",
                            "input": {"path": "README.md"}
                        }]
                    }
                }]
            }
        }))
        .expect("request fixture should deserialize");

        // When: a signature-error fallback request is derived.
        let fallback = request
            .without_reasoning_content()
            .expect("signed history should produce a fallback");
        let fallback_json = serde_json::to_value(fallback).expect("fallback should serialize");
        let original_json = serde_json::to_value(request).expect("original should serialize");

        // Then: only the machine reasoning field is removed from the clone.
        let assistant =
            &fallback_json["conversationState"]["history"][0]["assistantResponseMessage"];
        assert!(assistant.get("reasoningContent").is_none());
        assert_eq!(assistant["content"], "answer");
        assert_eq!(assistant["toolUses"][0]["toolUseId"], "toolu_1");
        assert_eq!(
            fallback_json["conversationState"]["currentMessage"],
            original_json["conversationState"]["currentMessage"]
        );
        assert_eq!(
            original_json["conversationState"]["history"][0]["assistantResponseMessage"]["reasoningContent"]
                ["reasoningText"]["signature"],
            "sig"
        );
    }
}
