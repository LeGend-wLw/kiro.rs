# Kiro Native PDF Support Implementation Plan

> **For agentic workers:** Implement task-by-task and verify each step before continuing.

**Goal:** Add native PDF document forwarding from the Anthropic-compatible API to KiroRS's Kiro 1.0.165 upstream schema.

**Architecture:** Keep Anthropic input open-ended, isolate PDF decoding/validation in a small document helper, and add `documents` to both current and historical Kiro user messages. The converter will aggregate documents across the conversation, enforce Kiro's five-document limit and duplicate-name rule, then serialize inline Base64 bytes in Kiro's native `{name, format, source.bytes}` shape.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, existing Base64-compatible standard library decoding strategy or a minimal existing dependency, Cargo tests, `cargo fmt`, `cargo clippy` where available.

---

### Task 1: Add failing document conversion tests

**Files:**
- Modify: `src/anthropic/converter.rs`
- Modify: `src/kiro/model/requests/conversation.rs`

- [ ] Add tests for a current user PDF block, a historical user PDF block, invalid Base64/PDF header, duplicate names, and six-document rejection.
- [ ] Run the focused converter tests and confirm they fail because `documents` is not modeled or forwarded.

### Task 2: Add Kiro document request types

**Files:**
- Modify: `src/kiro/model/requests/conversation.rs`

- [ ] Add `KiroDocument` and `KiroDocumentSource` with camelCase serialization and `format`, `name`, `source.bytes` fields.
- [ ] Add optional skipped-empty `documents` fields to `UserInputMessage` and historical `UserMessage`.
- [ ] Add `with_documents` builders and serialization tests matching the installed Kiro schema.

### Task 3: Parse and validate Anthropic PDFs

**Files:**
- Modify: `src/anthropic/converter.rs`
- Modify: `src/anthropic/types.rs`

- [ ] Add a helper that accepts a document content block, requires an inline Base64 source, validates `%PDF` magic bytes, uses its non-empty title or `document.pdf`, and returns `KiroDocument`.
- [ ] Reject URL/provider-reference sources and unsupported MIME types with a conversion error that handlers expose as `invalid_request_error`.
- [ ] Preserve existing image and tool-result parsing behavior.

### Task 4: Forward documents through current and history conversion

**Files:**
- Modify: `src/anthropic/converter.rs`

- [ ] Extend message-content processing to return documents alongside text, images, and tool results.
- [ ] Attach documents to the current `UserInputMessage` and historical `UserMessage` values.
- [ ] Enforce unique names and a maximum of five documents across current plus history.
- [ ] Rerun focused tests and then the full test suite.

### Task 5: Validate and manually exercise the service

**Files:**
- No source changes expected.

- [ ] Run `cargo fmt --check`, `cargo test --all-targets`, and Rust diagnostics on changed files.
- [ ] Build the release binary and replace only the running KiroRS process after confirming its current PID.
- [ ] Send a real PDF through OpenCode using `Mify-Claude/Claude Opus 4.8` and verify the returned text contains the PDF-only token.
- [ ] Send one malformed PDF and verify KiroRS returns a structured `400 invalid_request_error` rather than forwarding invalid bytes.
