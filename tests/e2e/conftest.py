from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path

import pytest
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
UNREADABLE_WORKSPACE = TEST_DIR / "project_unreadable"
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


async def wait_for_log_message(client: LanguageClient, prefix: str) -> None:
    def found_message() -> bool:
        return any(
            message.message.startswith(prefix) for message in client.log_messages
        )

    while not found_message():
        try:
            await wait_for_notification(client, types.WINDOW_LOG_MESSAGE)
        except TimeoutError as exc:
            if found_message():
                return
            raise AssertionError(
                f"Timed out waiting for log message: {prefix}"
            ) from exc


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


@pytest_lsp.fixture(config=ClientServerConfig(server_command=SERVER_COMMAND))
async def unreadable_client(lsp_client: LanguageClient):
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("visual-studio-code"),
            workspace_folders=[
                WorkspaceFolder(
                    uri=UNREADABLE_WORKSPACE.as_uri(), name="unreadable_project"
                )
            ],
        )
    )
    await wait_for_project_load(lsp_client)

    yield

    await lsp_client.shutdown_session()


@pytest_asyncio.fixture
async def client(vscode_client):
    yield vscode_client


@pytest_asyncio.fixture
async def isolated_bundled_navigation_client(tmp_path: Path):
    if sys.platform != "linux":
        pytest.skip(
            "isolated bundle-cache coverage currently configures XDG_CACHE_HOME"
        )

    project = tmp_path / "project"
    project.mkdir()
    (project / "settings.py").write_text(
        "INSTALLED_APPS = ['django.contrib.admin']\n"
        "TEMPLATES = [{'BACKEND': "
        "'django.template.backends.django.DjangoTemplates', 'APP_DIRS': True}]\n",
        encoding="utf-8",
    )
    template = project / "page.html"
    template.write_text(
        "{% for item in items %}{{ item }}{% endfor %}\n"
        "{% include 'admin/base.html' %}\n",
        encoding="utf-8",
    )
    cache = tmp_path / "cache"
    server_env = os.environ.copy()
    server_env["XDG_CACHE_HOME"] = str(cache)
    config = ClientServerConfig(server_command=SERVER_COMMAND, server_env=server_env)
    lsp_client = await config.start()
    await lsp_client.initialize_session(
        InitializeParams(
            capabilities=client_capabilities("visual-studio-code"),
            initialization_options={
                "django_settings_module": "settings",
                "django_version": "6.0",
                "venv_path": str(project / "missing-venv"),
            },
            workspace_folders=[WorkspaceFolder(uri=project.as_uri(), name="project")],
        )
    )
    await wait_for_project_load(lsp_client)

    yield lsp_client, template, cache / "djls" / "django"

    await lsp_client.shutdown_session()
