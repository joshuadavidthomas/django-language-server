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


async def wait_for_project_load(client: LanguageClient) -> None:
    def progress_done() -> bool:
        return any(
            event.kind == "end"
            for events in client.progress_reports.values()
            for event in events
        )

    while not progress_done():
        await asyncio.wait_for(client.wait_for_notification(types.PROGRESS), timeout=5)


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
