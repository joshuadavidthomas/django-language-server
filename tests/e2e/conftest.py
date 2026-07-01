from __future__ import annotations

import asyncio
from pathlib import Path

import pytest_asyncio
import pytest_lsp
from lsprotocol import types
from lsprotocol.types import InitializeParams
from lsprotocol.types import WorkspaceFolder
from pytest_lsp import ClientServerConfig
from pytest_lsp import LanguageClient
from pytest_lsp import client_capabilities

SERVER_COMMAND = [
    "cargo",
    "run",
    "-q",
    "-p",
    "djls",
    "--",
    "serve",
    "--connection-type",
    "stdio",
]
TEST_DIR = Path(__file__).parent.parent
TEST_WORKSPACE = TEST_DIR / "project"
EXPECTED_STARTUP_PROGRESS_TITLES = {
    "Resolving Django environment",
    "Discovering Django project facts",
    "Warming Django caches",
}


async def wait_for_notification(
    client: LanguageClient,
    method: str,
    timeout: float = 5,
) -> None:
    future = asyncio.wrap_future(client.protocol.wait_for_notification(method))
    await asyncio.wait_for(asyncio.shield(future), timeout=timeout)


async def wait_for_project_load(client: LanguageClient) -> None:
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

    while not EXPECTED_STARTUP_PROGRESS_TITLES <= completed_titles():
        try:
            await wait_for_notification(client, types.PROGRESS)
        except TimeoutError as exc:
            observed_titles = completed_titles()
            missing_titles = EXPECTED_STARTUP_PROGRESS_TITLES - observed_titles
            raise AssertionError(
                f"Timed out waiting for project load: {sorted(missing_titles)}; "
                f"observed: {sorted(observed_titles)}"
            ) from exc


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def emacs_client(lsp_client: LanguageClient):
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("emacs"),
            workspace_folders=[
                WorkspaceFolder(uri=TEST_WORKSPACE.as_uri(), name="test_project")
            ],
        )
    )
    await wait_for_project_load(lsp_client)

    yield

    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def neovim_client(lsp_client: LanguageClient):
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("neovim"),
            workspace_folders=[
                WorkspaceFolder(uri=TEST_WORKSPACE.as_uri(), name="test_project")
            ],
        )
    )
    await wait_for_project_load(lsp_client)

    yield

    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def vscode_client(lsp_client: LanguageClient):
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("visual-studio-code"),
            workspace_folders=[
                WorkspaceFolder(uri=TEST_WORKSPACE.as_uri(), name="test_project")
            ],
        )
    )
    await wait_for_project_load(lsp_client)

    yield

    await lsp_client.shutdown_session()


@pytest_asyncio.fixture
async def client(vscode_client):
    yield vscode_client
