from __future__ import annotations

import asyncio

import pytest
from lsprotocol import types
from pytest_lsp import LanguageClient


@pytest.mark.asyncio
async def test_client_initializes(client: LanguageClient):
    assert client.error is None
    assert client.capabilities is not None

    messages = [message.message for message in client.log_messages]

    assert "Initializing server..." in messages


@pytest.mark.asyncio
async def test_client_receives_initialized_notification(client: LanguageClient):
    while not any(
        message.message.startswith("Server initialization completed")
        for message in client.log_messages
    ):
        await asyncio.wait_for(
            client.wait_for_notification(types.WINDOW_LOG_MESSAGE),
            timeout=5,
        )

    messages = [message.message for message in client.log_messages]

    assert "Server received initialized notification." in messages
    assert any(
        message.startswith("Server initialization completed") for message in messages
    )
