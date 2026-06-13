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


async def wait_for_log_message(client: LanguageClient, prefix: str) -> None:
    while not any(message.message.startswith(prefix) for message in client.log_messages):
        await asyncio.wait_for(
            client.wait_for_notification(types.WINDOW_LOG_MESSAGE),
            timeout=5,
        )


async def wait_for_progress_events(
    client: LanguageClient,
) -> list[
    types.WorkDoneProgressBegin
    | types.WorkDoneProgressReport
    | types.WorkDoneProgressEnd
]:
    def events() -> list[
        types.WorkDoneProgressBegin
        | types.WorkDoneProgressReport
        | types.WorkDoneProgressEnd
    ]:
        return [event for events in client.progress_reports.values() for event in events]

    while not any(event.kind == "end" for event in events()):
        await asyncio.wait_for(client.wait_for_notification(types.PROGRESS), timeout=5)

    return events()


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
async def test_server_accepts_template_requests_after_initialized(
    startup_client: LanguageClient,
):
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
    events = await wait_for_progress_events(startup_client)

    assert startup_client.progress_reports, "window/workDoneProgress/create was not observed"
    assert [event.kind for event in events][0] == "begin"
    assert any(event.kind == "report" for event in events)
    assert [event.kind for event in events][-1] == "end"
    assert any(
        isinstance(event, types.WorkDoneProgressBegin)
        and event.title == "Loading Django project"
        for event in events
    )


@pytest.mark.asyncio
async def test_unsupported_client_receives_log_fallback(
    no_progress_client: LanguageClient,
):
    await wait_for_log_message(no_progress_client, "Loading Django project")
    await wait_for_log_message(no_progress_client, "Server initialization completed")

    messages = [message.message for message in no_progress_client.log_messages]

    assert not no_progress_client.progress_reports
    assert "Loading Django project" in messages
    assert any(
        message.startswith("Loading Django project: Warming caches")
        for message in messages
    )
