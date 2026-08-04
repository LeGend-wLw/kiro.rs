//! Kiro 流式响应启动阶段的预取与瞬态错误识别。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};

use crate::kiro::model::events::Event;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::kiro::provider::KiroProvider;

const TRANSIENT_UPSTREAM_ERROR: &str =
    "Encountered an unexpected error when processing the request, please try again.";
const STREAM_START_ATTEMPTS: usize = 3;
const MAX_PREFETCH_BYTES: usize = 1024 * 1024;

type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>;
type RetryFuture = Pin<Box<dyn Future<Output = anyhow::Result<reqwest::Response>> + Send>>;
type RetryRequest = Arc<dyn Fn() -> RetryFuture + Send + Sync>;

pub struct KiroStreamResponse {
    initial_response: reqwest::Response,
    retry_request: RetryRequest,
}

struct ProbeState {
    stream: UpstreamStream,
    decoder: EventStreamDecoder,
    prefetched: Vec<Bytes>,
    prefetched_bytes: usize,
}

enum StreamMode {
    Probe(ProbeState),
    Replay(std::vec::IntoIter<Bytes>, UpstreamStream),
    Pass(UpstreamStream),
    Finished,
}

struct RetryStreamState {
    mode: StreamMode,
    retry_request: RetryRequest,
    attempt: usize,
}

enum ProbeDecision {
    Continue,
    Ready,
    Retry,
}

impl ProbeState {
    fn new(response: reqwest::Response) -> Self {
        Self {
            stream: Box::pin(response.bytes_stream()),
            decoder: EventStreamDecoder::new(),
            prefetched: Vec::new(),
            prefetched_bytes: 0,
        }
    }

    fn push(&mut self, chunk: Bytes) -> anyhow::Result<ProbeDecision> {
        self.prefetched_bytes = self
            .prefetched_bytes
            .checked_add(chunk.len())
            .filter(|size| *size <= MAX_PREFETCH_BYTES)
            .context("Kiro 流首事件前的数据超过 1 MiB")?;
        self.decoder
            .feed(&chunk)
            .context("解码 Kiro 流首事件失败")?;
        self.prefetched.push(chunk);

        for frame in self.decoder.decode_iter() {
            let event = Event::from_frame(frame.context("解码 Kiro 流首帧失败")?)
                .context("解析 Kiro 流首事件失败")?;
            if is_retryable_start_event(&event) {
                return Ok(ProbeDecision::Retry);
            }
            if is_terminal_start_event(&event) {
                return Ok(ProbeDecision::Ready);
            }
        }
        Ok(ProbeDecision::Continue)
    }

    fn into_replay(self) -> StreamMode {
        StreamMode::Replay(self.prefetched.into_iter(), self.stream)
    }
}

impl RetryStreamState {
    fn new(response: reqwest::Response, retry_request: RetryRequest) -> Self {
        Self {
            mode: StreamMode::Probe(ProbeState::new(response)),
            retry_request,
            attempt: 1,
        }
    }

    async fn restart(&mut self) -> anyhow::Result<ProbeState> {
        tokio::time::sleep(KiroProvider::retry_delay(self.attempt - 1)).await;
        let response = (self.retry_request)().await?;
        self.attempt += 1;
        Ok(ProbeState::new(response))
    }

    fn can_retry(&self) -> bool {
        self.attempt < STREAM_START_ATTEMPTS
    }
}

impl KiroStreamResponse {
    pub(super) fn new(
        provider: Arc<KiroProvider>,
        initial_response: reqwest::Response,
        request_body: &str,
        fallback_request_body: Option<&str>,
    ) -> Self {
        let request_body: Arc<str> = Arc::from(request_body);
        let fallback_request_body: Option<Arc<str>> = fallback_request_body.map(Arc::from);
        let retry_request = Arc::new(move || {
            let provider = provider.clone();
            let request_body = request_body.clone();
            let fallback_request_body = fallback_request_body.clone();
            Box::pin(async move {
                provider
                    .call_api_with_retry(&request_body, fallback_request_body.as_deref(), true)
                    .await
            }) as RetryFuture
        });
        Self {
            initial_response,
            retry_request,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_retry_request<F, Fut, E>(
        initial_response: reqwest::Response,
        request: F,
    ) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<reqwest::Response, E>> + Send + 'static,
        E: Into<anyhow::Error> + 'static,
    {
        let retry_request = Arc::new(move || {
            let future = request();
            Box::pin(async move { future.await.map_err(Into::into) }) as RetryFuture
        });
        Self {
            initial_response,
            retry_request,
        }
    }

    pub fn bytes_stream(
        self,
    ) -> Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send + 'static>> {
        let state = RetryStreamState::new(self.initial_response, self.retry_request);
        Box::pin(stream::unfold(state, |mut state| async move {
            loop {
                let mode = std::mem::replace(&mut state.mode, StreamMode::Finished);
                match mode {
                    StreamMode::Probe(mut probe) => match probe.stream.next().await {
                        Some(Ok(chunk)) => match probe.push(chunk) {
                            Ok(ProbeDecision::Continue) => state.mode = StreamMode::Probe(probe),
                            Ok(ProbeDecision::Ready) => state.mode = probe.into_replay(),
                            Ok(ProbeDecision::Retry) if state.can_retry() => {
                                tracing::warn!(
                                    attempt = state.attempt,
                                    max_attempts = STREAM_START_ATTEMPTS,
                                    "Kiro 流启动时返回瞬态异常，正在重试"
                                );
                                match state.restart().await {
                                    Ok(next) => state.mode = StreamMode::Probe(next),
                                    Err(error) => return Some((Err(error), state)),
                                }
                            }
                            Ok(ProbeDecision::Retry) => state.mode = probe.into_replay(),
                            Err(error) => return Some((Err(error), state)),
                        },
                        Some(Err(error)) if state.can_retry() => {
                            tracing::warn!(
                                attempt = state.attempt,
                                max_attempts = STREAM_START_ATTEMPTS,
                                error = %error,
                                "Kiro 流首事件读取失败，正在重试"
                            );
                            match state.restart().await {
                                Ok(next) => state.mode = StreamMode::Probe(next),
                                Err(error) => return Some((Err(error), state)),
                            }
                        }
                        Some(Err(error)) => return Some((Err(error.into()), state)),
                        None if state.can_retry() => match state.restart().await {
                            Ok(next) => state.mode = StreamMode::Probe(next),
                            Err(error) => return Some((Err(error), state)),
                        },
                        None => {
                            let error = match probe.decoder.finish() {
                                Ok(()) => {
                                    anyhow::anyhow!("Kiro stream ended before its first event")
                                }
                                Err(error) => {
                                    anyhow::anyhow!("Kiro response stream was truncated: {error}")
                                }
                            };
                            return Some((Err(error), state));
                        }
                    },
                    StreamMode::Replay(mut chunks, stream) => match chunks.next() {
                        Some(chunk) => {
                            state.mode = StreamMode::Replay(chunks, stream);
                            return Some((Ok(chunk), state));
                        }
                        None => state.mode = StreamMode::Pass(stream),
                    },
                    StreamMode::Pass(mut stream) => match stream.next().await {
                        Some(Ok(chunk)) => {
                            state.mode = StreamMode::Pass(stream);
                            return Some((Ok(chunk), state));
                        }
                        Some(Err(error)) => return Some((Err(error.into()), state)),
                        None => return None,
                    },
                    StreamMode::Finished => return None,
                }
            }
        }))
    }
}

fn is_retryable_start_event(event: &Event) -> bool {
    match event {
        Event::Error { error_message, .. } => is_exact_transient_message(error_message),
        Event::Exception { message, .. } => is_exact_transient_message(message),
        Event::AssistantResponse(_)
        | Event::ToolUse(_)
        | Event::Metering(())
        | Event::ContextUsage(_)
        | Event::ReasoningContent(_)
        | Event::Unknown {} => false,
    }
}

fn is_exact_transient_message(payload: &str) -> bool {
    if payload == TRANSIENT_UPSTREAM_ERROR {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
        .is_some_and(|message| message == TRANSIENT_UPSTREAM_ERROR)
}

fn is_terminal_start_event(event: &Event) -> bool {
    matches!(
        event,
        Event::AssistantResponse(_)
            | Event::ToolUse(_)
            | Event::ReasoningContent(_)
            | Event::Error { .. }
            | Event::Exception { .. }
    )
}

#[cfg(test)]
#[path = "stream_response_tests.rs"]
mod tests;
