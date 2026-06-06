from __future__ import annotations

import pytest_asyncio
import pytest_lsp
from lsprotocol.types import InitializeParams
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

# LSP clients supported by pytest-lsp
CLIENTS = [
    "emacs_v29.1",
    "neovim_v0.6.1",
    "neovim_v0.7.0",
    "neovim_v0.8.0",
    "neovim_v0.9.1",
    "neovim_v0.10.0",
    "neovim_v0.11.0",
    "visual_studio_code_v1.65.2",
]


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def emacs_client(lsp_client: LanguageClient, tmp_path):
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("emacs"),
            root_uri=tmp_path.as_uri(),
        )
    )

    yield

    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def neovim_client(lsp_client: LanguageClient, tmp_path):
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("neovim"),
            root_uri=tmp_path.as_uri(),
        )
    )

    yield

    await lsp_client.shutdown_session()


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def vscode_client(lsp_client: LanguageClient, tmp_path):
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("visual-studio-code"),
            root_uri=tmp_path.as_uri(),
        )
    )

    yield

    await lsp_client.shutdown_session()


@pytest_asyncio.fixture
async def client(vscode_client):
    yield vscode_client
