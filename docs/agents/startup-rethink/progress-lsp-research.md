# Technical Research: LSP progress for startup-rethink

## Summary
- LSP progress is the generic `$/progress` notification, introduced in LSP 3.15. It carries either work-done progress payloads for user-visible long-running work or partial-result payloads for streaming request results.
- Server-originated background work uses `window/workDoneProgress/create` plus `$/progress` begin/report/end payloads, gated by the client capability `window.workDoneProgress`.
- rust-analyzer uses server-initiated work-done progress for startup and background work such as workspace fetching, VFS/root scanning, crate graph construction, proc-macro/build-data loading, indexing, project discovery, and flycheck.
- `tower-lsp-server` 0.23.0 exposes the required `ls_types` data structures, `Client::create_work_done_progress`, and a `Client::progress` notification builder. The builder only sends `$/progress`; it does not create server-initiated progress tokens.
- DJLS currently does not emit work-done progress, does not record `window.workDoneProgress`, and does not handle `window/workDoneProgress/cancel`. Startup/readiness information is currently visible through tracing logs forwarded as `window/logMessage`.

## LSP progress feature

### Answer
The LSP progress mechanism is token-based. A progress token is separate from a request ID, and progress updates are sent as `$/progress` notifications carrying a token plus a payload. For work-done progress, the payload is one of `begin`, `report`, or `end`. Work-done progress can be attached to a client request through a `workDoneToken`, or initiated by the server through `window/workDoneProgress/create`. Partial results also use `$/progress`, but use a `partialResultToken` and request-specific result payloads rather than the work-done begin/report/end UI payloads.

For server-initiated work-done progress, the client must advertise `window.workDoneProgress`. The server sends `window/workDoneProgress/create` with a token; if that request errors, the server must not send progress notifications for the token. The token is single-use for one begin, zero or more reports, and one end. Cancellation of server-initiated work-done progress arrives as `window/workDoneProgress/cancel` with the token.

During `initialize`, the spec forbids server requests and notifications until the initialize response, except that a server may use the `workDoneToken` supplied in the initialize params, and only that token, for `$/progress` notifications.

### Findings
- `microsoft/language-server-protocol:_specifications/lsp/3.17/specification.md:360-390` — LSP 3.17 defines `ProgressToken = integer | string` and `ProgressParams<T> { token, value }`; progress tokens are distinct from request IDs.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/types/workDoneProgress.md:1-105` — work-done progress uses `$/progress` and has `WorkDoneProgressBegin`, `WorkDoneProgressReport`, and `WorkDoneProgressEnd` payloads. `begin.title` is required; `percentage` is optional and has range `[0, 100]`.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/types/workDoneProgress.md:108-147` — work-done progress has two initiation modes: request sender supplies `workDoneToken`, or server requests `window/workDoneProgress/create`.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/types/workDoneProgress.md:165-188` — request-attached work-done progress tokens are valid only until the request response; cancellation is done by canceling the request. Servers advertise request-specific support with `WorkDoneProgressOptions { workDoneProgress?: boolean }`.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/types/workDoneProgress.md:189-204` — server-initiated work-done progress is allowed only when the client advertises `window.workDoneProgress`; the token should be used once for one begin, many reports, and one end.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/window/workDoneProgressCreate.md:1-27` — `window/workDoneProgress/create` takes `WorkDoneProgressCreateParams { token }` and returns `null`; if it errors, the server must not send progress for that token.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/window/workDoneProgressCancel.md:1-18` — `window/workDoneProgress/cancel` sends `WorkDoneProgressCancelParams { token }` from client to server, and progress need not have been marked cancellable to be canceled.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/types/partialResults.md:1-34` and `types/partialResultParams.md:1-14` — partial-result progress is also `$/progress`, but is enabled by a request `partialResultToken`; if partial results are used, the final response is empty and errors determine whether partial results are usable.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/general/initialize.md:1-9` — before the initialize response, the only progress exception is using the initialize request's supplied progress token for `$/progress`.
- `microsoft/language-server-protocol:_specifications/lsp/3.17/general/initialize.md:470-486` — `ClientCapabilities.window.workDoneProgress` controls server-initiated progress and whether the client handles work-done progress notifications.

## rust-analyzer usage

### Answer
rust-analyzer centralizes LSP work-done progress in `GlobalState::report_progress`. It first checks client/config support for work-done progress, creates a string progress token, sends `window/workDoneProgress/create` on begin, and sends `$/progress` notifications with `WorkDoneProgress::{Begin, Report, End}`. The current source uses work-done progress for startup-adjacent and background tasks including workspace fetching, VFS/root scanning, crate graph construction, build-script/build-data loading, proc-macro loading, cache priming/indexing, project discovery, and flycheck.

rust-analyzer also handles `window/workDoneProgress/cancel`, but only acts on flycheck tokens matching its flycheck token pattern; other progress cancellation notifications are ignored.

### Findings
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/lsp/capabilities.rs:377-379` — work-done progress support is read from `ClientCapabilities.window.work_done_progress` and defaults to false.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/lsp/utils.rs:116-165` — `GlobalState::report_progress` returns early when work-done progress is disabled, creates a `ProgressToken::String`, sends `request::WorkDoneProgressCreate` on `Begin`, and sends `notification::Progress` with `ProgressParamsValue::WorkDone(...)`.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/main_loop.rs:191-200` — startup queues and starts workspace fetching.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:282-387` and `main_loop.rs:821-838` — workspace fetching reports `Begin`, `Report`, and `End` under the title `Fetching`.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/main_loop.rs:950-1015` — VFS/root scanning reports `Roots Scanned` progress with done/total messages and fractional percentage; reports are coalesced.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:741-812` — crate graph construction reports `Building CrateGraph` begin/end around graph construction.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/reload.rs:389-468` and `main_loop.rs:873-910` — build data and proc macro loading report `Building compile-time-deps` and `Loading proc-macros`.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/main_loop.rs:343-419` and `main_loop.rs:599-615` — cache priming/indexing reports `Indexing` with a cancellable token and coalesced crate-count reports.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/main_loop.rs:1067-1098` — project discovery reports progress using the configured progress label and ends with an error message when discovery fails.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/main_loop.rs:1197-1245` — flycheck/cargo check uses cancellable tokens such as `rust-analyzer/flycheck/{id}`.
- `rust-lang/rust-analyzer:crates/rust-analyzer/src/handlers/notification.rs:40-55` and `main_loop.rs:1382-1386` — rust-analyzer registers `WorkDoneProgressCancel` and honors only flycheck token cancellation.
- `rust-lang/rust-analyzer:crates/rust-analyzer/tests/slow-tests/support.rs:260-263` — slow-test client capabilities disable work-done progress in synthetic tests.

## tower-lsp-server and ls-types handling

### Answer
DJLS is pinned to `tower-lsp-server` 0.23.0. That crate re-exports `ls_types`, and the lockfile pins `ls-types` 0.0.2. In this version, `ls_types::ProgressToken` is `NumberOrString`, `ProgressParamsValue` contains a `WorkDone(WorkDoneProgress)` variant, and work-done progress payload structs match the LSP begin/report/end model.

`tower-lsp-server::Client` has two layers of support:

- `Client::create_work_done_progress(token)` sends `window/workDoneProgress/create` and returns `jsonrpc::Result<()>`.
- `Client::progress(token, title)` returns a builder that emits `$/progress` begin/report/end notifications for an existing token.

The `progress` builder does not call `create_work_done_progress`. That matches request-attached progress tokens directly, while server-initiated progress requires a separate create request before using the token. `tower-lsp-server` also gates client requests and notifications on its server state. It sets `State::Initialized` after a successful `initialize` response; before that state, `send_request` returns `not_initialized_error()` and `send_notification` suppresses the message. The crate's `LanguageServer` trait does not expose a `work_done_progress_cancel` callback in 0.23.0; the source has a TODO for that method.

### Findings
- `Cargo.toml:19-22` — the workspace pins `tower-lsp-server = { version = "0.23.0", features = ["proposed"] }`.
- `Cargo.lock:2003-2014` — the locked `ls-types` version is `0.0.2`.
- `Cargo.lock:3744-3762` — the locked `tower-lsp-server` version is `0.23.0` and depends on `ls-types`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/lib.rs:75-82` — `tower_lsp_server` publicly re-exports `ls_types` and progress builder types.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/lib.rs:122-143` and `src/progress.rs:5-17` — `ProgressToken` aliases `NumberOrString`, whose variants are `Number(i32)` and `String(String)` with `From` impls for `i32`, `&str`, and `String`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/progress.rs:18-23` — `ProgressParamsValue` is an untagged enum with `WorkDone(WorkDoneProgress)`; this pinned type source does not expose a generic typed partial-result payload variant.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/progress.rs:25-57` — `WorkDoneProgressCreateParams`, `WorkDoneProgressCancelParams`, `WorkDoneProgressOptions`, and `WorkDoneProgressParams` are defined, with `work_done_progress: Option<bool>` and `work_done_token: Option<ProgressToken>`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/progress.rs:61-134` — `WorkDoneProgressBegin`, `Report`, `End`, and enum `WorkDoneProgress` are defined with serde tag `kind` and lowercase variant names.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/window.rs:26-49` — `WindowClientCapabilities` has `work_done_progress: Option<bool>`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/lib.rs:954-957` — `InitializeParams` includes flattened `WorkDoneProgressParams` for initialization progress.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/document_diagnostic.rs:74-94` — `DocumentDiagnosticParams` includes flattened `WorkDoneProgressParams` and `PartialResultParams`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/request.rs:685-694` — `request::WorkDoneProgressCreate` maps to method `window/workDoneProgress/create`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/src/notification.rs:318-339` — `notification::Progress` maps to `$/progress`; `notification::WorkDoneProgressCancel` maps to `window/workDoneProgress/cancel`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/service/client.rs:414-438` — `Client::create_work_done_progress` wraps `send_request::<request::WorkDoneProgressCreate>(WorkDoneProgressCreateParams { token })` and documents that progress must not be sent on create error.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/service/client.rs:531-577` — `Client::progress` creates a `$/progress` stream builder for a `ProgressToken` and title.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/service/client/progress.rs:13-132` — `Progress` builder starts as unbounded and not cancellable; `.begin()` sends a `notification::Progress` with `WorkDoneProgress::Begin`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/service/client/progress.rs:143-372` — `OngoingProgress` report and finish methods send `WorkDoneProgress::Report` and `WorkDoneProgress::End` notifications.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/service/state.rs:9-17` — server state includes `Uninitialized`, `Initializing`, and `Initialized`; `Initialized` means the server responded successfully to `initialize`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/service/layers.rs:64-84` — the initialize middleware sets state to `Initialized` after a successful initialize response.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/service/client.rs:579-630` — `send_notification` only sends in `Initialized` or `ShutDown`; `send_request` otherwise returns `not_initialized_error()`.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/server.rs:71-77` — the router handles generic `$/cancelRequest` by canceling pending requests.
- `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/src/server.rs:1380-1395` — the `LanguageServer` trait source has a TODO to add `work_done_progress_cancel()` when supported.

## Current DJLS behavior

### Answer
The startup-rethink ticket and outline already expect phased readiness/progress/log visibility, but current DJLS code has no explicit work-done progress implementation. The only progress-related server capability currently found is `DiagnosticOptions.work_done_progress_options: WorkDoneProgressOptions::default()`, which leaves `work_done_progress` unset. `ClientInfo` records pull diagnostics and snippet support, but not `window.workDoneProgress`. The diagnostic request handler receives params that can contain `workDoneToken` and `partialResultToken`, but the current handler uses only `params.text_document`.

Current startup status is communicated through tracing. The logging layer forwards INFO-and-above tracing events to the LSP client as `window/logMessage`; it is not the LSP progress channel. The current `initialized` handler loads the template-library cache, queues `refresh_external_data`, and waits for that receiver when the cache was not loaded.

### Repo findings
- `docs/tickets/startup-rethink.md:32-38` — the success criteria require a model separating LSP readiness, cheap catalog construction, lazy semantic queries, and background enrichment.
- `docs/tickets/startup-rethink.md:57-66` — open questions include how partial readiness, degraded mode, cache seeding, and late-arriving Project Facts are exposed through the LSP layer.
- `docs/agents/startup-rethink/outline.md:4` — the outline states protocol readiness should be observable before Django discovery and background discovery must not block `initialize` or await `initialized`.
- `docs/agents/startup-rethink/plan.md` — the implementation plan is authoritative for phase mechanics: Phase 1 is protocol-only/no-project degradation, while startup progress and loading orchestration land with the later loading executor slices.
- `docs/agents/startup-rethink/outline.md:291-292` — Phase 9 says `Enriched` and degraded enrichment status are client-visible through work-done progress/logging.
- `docs/agents/startup-rethink/outline.md:316-317` — the planned e2e startup test expects `initialize`/`initialized`, a request while workspace loading is in progress, and a later readiness/progress/log update.
- `crates/djls-server/src/lib.rs:16-53` — the LSP entrypoint builds `LspService` and stores a closure that forwards tracing events with `client.log_message(...)`.
- `crates/djls-server/src/logging.rs:31-110` — `LspLayer` maps tracing levels to `ls_types::MessageType` and invokes the configured send-message closure.
- `crates/djls-server/src/logging.rs:129-171` — tracing is configured with file logging plus the LSP forwarding layer filtered to INFO level.
- `crates/djls-server/src/server.rs:130-196` — `initialize` constructs `Session::new(&params)`, installs the session, and returns `InitializeResult` capabilities.
- `crates/djls-server/src/server.rs:173-179` — diagnostic capabilities include `work_done_progress_options: ls_types::WorkDoneProgressOptions::default()`.
- `crates/djls-server/src/server.rs:199-250` — `initialized` loads a template-library cache, queues `refresh_external_data`, and awaits the queued receiver when no cache was loaded.
- `crates/djls-server/src/session.rs:49-87` — `Session::new` reads workspace roots and client options, loads settings, constructs `DjangoDatabase`, and records `ClientInfo`.
- `crates/djls-server/src/client.rs:95-122` — `ClientCapabilities` stores only `pull_diagnostics` and `snippets`; there is no `window.workDoneProgress` handling.
- `crates/djls-server/src/server.rs:386-423` — the diagnostic handler accepts `DocumentDiagnosticParams`, uses `params.text_document`, collects diagnostics, and returns a full report; no work-done or partial-result tokens are read.
- `crates/djls-server/src/server.rs:173-179` and `crates/djls-server/src` search evidence — no calls were found to `create_work_done_progress`, `Client::progress`, `send_notification::<notification::Progress>`, or direct `WorkDoneProgress` payload construction.
- `crates/djls-server/src/server.rs:173-180` and repo search evidence — no `window/workDoneProgress/cancel` or `WorkDoneProgressCancel` handler was found under `crates/`.

### Tests
- `docs/agents/startup-rethink/design.md:241-252` — the design lists startup/LSP tests and a tiny e2e startup/readiness contract as required future coverage.
- `docs/agents/startup-rethink/outline.md:316-317` — the outline names a planned `tests/lsp/test_startup.py` e2e test.
- `tests/` search evidence — no existing LSP startup/readiness/progress test was found, and `crates/djls-server/tests` is absent in this checkout.

### Gaps
- I found no current DJLS code that emits `$/progress`, creates a server-initiated work-done progress token, or stores the client `window.workDoneProgress` capability.
- I found no current DJLS handler for `window/workDoneProgress/cancel`.
- I found no current DJLS use of request-attached `workDoneToken` or `partialResultToken` in diagnostic requests.
- The pinned `ls-types` 0.0.2 `ProgressParamsValue` source only exposes a `WorkDone` variant; typed partial-result progress payload support was not evident in the pinned dependency source.

## Sources
- LSP 3.17 current spec: `https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/specification.md`
- LSP work-done progress spec: `https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/types/workDoneProgress.md`
- LSP work-done progress create/cancel specs: `https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/window/workDoneProgressCreate.md`, `https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/window/workDoneProgressCancel.md`
- LSP partial-results spec: `https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/types/partialResults.md`
- rust-analyzer current source at research time: `https://github.com/rust-lang/rust-analyzer`
- `tower-lsp-server` 0.23.0 local registry source: `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tower-lsp-server-0.23.0/`
- `ls-types` 0.0.2 local registry source: `/home/josh/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ls-types-0.0.2/`
