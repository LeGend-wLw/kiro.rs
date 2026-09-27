# Kiro Native PDF Support Design

**Goal:** Preserve Anthropic PDF document blocks and send them to Kiro's native `userInputMessage.documents` field.

**Decision:** Kiro 1.0.165's installed agent bundle defines `documents` as document blocks with `name`, `format`, and binary `source.bytes`; PDF is mapped from `application/pdf` to `format: "pdf"`. Kiro validates PDF magic bytes and allows at most five documents per conversation. KiroRS will use this native shape rather than extracting text or rasterizing pages.

**Data flow:** Anthropic `document` content block -> validate source and PDF bytes -> `KiroDocument` -> current/history `userInputMessage.documents` -> Kiro `GenerateAssistantResponse`.

**Error policy:** Unsupported document source types, invalid Base64, invalid PDF magic bytes, duplicate names, and more than five conversation documents become `invalid_request_error` responses. URL sources are rejected because Kiro's native request requires inline bytes and fetching arbitrary URLs would add SSRF risk.

**Testing:** Unit tests cover PDF parsing, Kiro JSON serialization, current-message conversion, history conversion, duplicate/count validation, and rejection cases. Manual QA sends a real PDF through the running KiroRS Anthropic endpoint and verifies the upstream model returns a token embedded only in the PDF.
