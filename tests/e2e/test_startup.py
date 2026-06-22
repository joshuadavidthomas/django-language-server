from __future__ import annotations

import asyncio

import pytest
import pytest_lsp
from lsprotocol import types
from pytest_lsp import ClientServerConfig
from pytest_lsp import LanguageClient
from pytest_lsp import client_capabilities

from .conftest import SERVER_COMMAND
from .conftest import TEST_WORKSPACE
from .utils import position_after

BASE_TEMPLATE = TEST_WORKSPACE / "djls_app" / "templates" / "djls_app" / "base.html"
EXPECTED_STARTUP_PROGRESS_TITLES = {
    "Resolving Django environment",
    "Discovering Django project facts",
    "Warming Django caches",
    "Publishing diagnostics",
}


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def startup_client(lsp_client: LanguageClient):
    initialize_result = await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=client_capabilities("visual-studio-code"),
            workspace_folders=[
                types.WorkspaceFolder(
                    uri=TEST_WORKSPACE.as_uri(),
                    name="test_project",
                )
            ],
        )
    )
    lsp_client.djls_initialize_result = initialize_result

    yield lsp_client

    await lsp_client.shutdown_session()


def no_progress_capabilities() -> types.ClientCapabilities:
    capabilities = client_capabilities("visual-studio-code")
    if capabilities.window is None:
        capabilities.window = types.WindowClientCapabilities()
    capabilities.window.work_done_progress = False
    return capabilities


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def no_progress_client(lsp_client: LanguageClient):
    await lsp_client.initialize_session(
        types.InitializeParams(
            capabilities=no_progress_capabilities(),
            workspace_folders=[
                types.WorkspaceFolder(
                    uri=TEST_WORKSPACE.as_uri(),
                    name="test_project",
                )
            ],
        )
    )

    yield lsp_client

    await lsp_client.shutdown_session()


async def wait_for_notification(
    client: LanguageClient,
    method: str,
    timeout: float = 5,
) -> None:
    future = asyncio.wrap_future(client.protocol.wait_for_notification(method))
    await asyncio.wait_for(asyncio.shield(future), timeout=timeout)


async def wait_for_log_message(client: LanguageClient, prefix: str) -> None:
    def found_message() -> bool:
        return any(message.message.startswith(prefix) for message in client.log_messages)

    while not found_message():
        try:
            await wait_for_notification(client, types.WINDOW_LOG_MESSAGE)
        except TimeoutError as exc:
            if found_message():
                return
            raise AssertionError(f"Timed out waiting for log message: {prefix}") from exc


async def wait_for_progress_titles(
    client: LanguageClient,
    expected_titles: set[str],
) -> dict[
    object,
    list[
        types.WorkDoneProgressBegin
        | types.WorkDoneProgressReport
        | types.WorkDoneProgressEnd
    ],
]:
    def completed_titles() -> set[str]:
        titles = set()
        for events in client.progress_reports.values():
            begin = next(
                (
                    event
                    for event in events
                    if isinstance(event, types.WorkDoneProgressBegin)
                ),
                None,
            )
            if begin is None:
                continue
            if any(isinstance(event, types.WorkDoneProgressEnd) for event in events):
                titles.add(begin.title)
        return titles

    while not expected_titles <= completed_titles():
        try:
            await wait_for_notification(client, types.PROGRESS)
        except TimeoutError as exc:
            observed_titles = completed_titles()
            missing_titles = expected_titles - observed_titles
            raise AssertionError(
                f"Timed out waiting for progress titles: {sorted(missing_titles)}; "
                f"observed: {sorted(observed_titles)}"
            ) from exc

    return client.progress_reports


@pytest.mark.asyncio
async def test_initialize_returns_protocol_capabilities_without_project_loading(
    startup_client: LanguageClient,
):
    capabilities = startup_client.djls_initialize_result.capabilities

    assert startup_client.error is None
    assert capabilities is not None
    assert capabilities.text_document_sync is not None
    assert capabilities.completion_provider is not None
    assert capabilities.diagnostic_provider is not None


@pytest.mark.asyncio
async def test_server_accepts_template_requests_after_startup_load(
    startup_client: LanguageClient,
):
    await wait_for_progress_titles(startup_client, EXPECTED_STARTUP_PROGRESS_TITLES)

    startup_client.text_document_did_open(
        types.DidOpenTextDocumentParams(
            text_document=types.TextDocumentItem(
                uri=BASE_TEMPLATE.as_uri(),
                language_id="htmldjango",
                version=1,
                text=BASE_TEMPLATE.read_text(encoding="utf-8"),
            )
        )
    )

    result = await startup_client.text_document_completion_async(
        types.CompletionParams(
            text_document=types.TextDocumentIdentifier(uri=BASE_TEMPLATE.as_uri()),
            position=position_after(BASE_TEMPLATE, "{% sta"),
        )
    )

    assert result is not None


@pytest.mark.asyncio
async def test_supported_client_receives_startup_progress_begin_report_end(
    startup_client: LanguageClient,
):
    progress_reports = await wait_for_progress_titles(
        startup_client,
        EXPECTED_STARTUP_PROGRESS_TITLES,
    )
    events = [event for events in progress_reports.values() for event in events]

    assert progress_reports, "window/workDoneProgress/create was not observed"
    assert len(progress_reports) >= 2
    assert any(event.kind == "report" for event in events)

    titles_by_token = {}
    for token, token_events in progress_reports.items():
        begin_index = next(
            (
                index
                for index, event in enumerate(token_events)
                if isinstance(event, types.WorkDoneProgressBegin)
            ),
            None,
        )
        if begin_index is None:
            continue
        end_index = next(
            (
                index
                for index, event in enumerate(token_events)
                if isinstance(event, types.WorkDoneProgressEnd)
            ),
            None,
        )
        assert end_index is not None
        assert begin_index < end_index
        titles_by_token[token] = token_events[begin_index].title

    observed_titles = set(titles_by_token.values())
    assert EXPECTED_STARTUP_PROGRESS_TITLES <= observed_titles
    assert "Loading Django project" not in observed_titles

    report_messages = [
        event.message
        for event in events
        if isinstance(event, types.WorkDoneProgressReport)
    ]
    for expected in [
        "Resolving environment",
        "Scanning settings",
        "Discovering model modules",
        "Discovering template libraries",
        "Discovering template tag candidates",
        "Applying project facts",
        "Building tag specs",
        "Building filter arity specs",
        "Building model graph",
        "Resolving template directories",
        "Indexing template libraries",
        "Indexing templates",
        "Publishing diagnostics",
    ]:
        assert expected in report_messages


@pytest.mark.asyncio
async def test_unsupported_client_receives_log_fallback(
    no_progress_client: LanguageClient,
):
    await wait_for_log_message(no_progress_client, "Resolving Django environment")
    await wait_for_log_message(no_progress_client, "Server initialization completed")

    messages = [message.message for message in no_progress_client.log_messages]

    assert not no_progress_client.progress_reports
    for expected in EXPECTED_STARTUP_PROGRESS_TITLES:
        assert expected in messages
    assert any(
        message.startswith(
            "Discovering Django project facts: Discovering template libraries"
        )
        for message in messages
    )
    assert any(
        message.startswith("Warming Django caches: Indexing templates")
        for message in messages
    )
