from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import sys
from pathlib import Path

logger = logging.getLogger(__name__)


class Agent:
    def __init__(self, ipc_path: Path):
        self.ipc_path = ipc_path

    async def start(self):
        if sys.platform == "win32":
            # Windows named pipe
            pipe_path = rf"\\.\pipe\{self.ipc_path.name}"
            server = await asyncio.start_server(
                self.handle_client,
                pipe=pipe_path,
            )
        else:
            # Unix domain socket
            try:
                self.ipc_path.unlink()
            except FileNotFoundError:
                pass

            server = await asyncio.start_unix_server(
                self.handle_client,
                path=str(self.ipc_path),
            )

        async with server:
            await server.serve_forever()

    async def handle_client(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ):
        try:
            while True:
                data = await reader.readline()
                if not data:
                    break

                request = json.loads(data)
                response = await self.handle_request(request)

                writer.write(json.dumps(response).encode() + b"\n")
                await writer.drain()
        except Exception:
            logger.exception("Error handling client request")
        finally:
            writer.close()
            await writer.wait_closed()

    async def handle_request(self, request): ...


def main():
    logging.basicConfig(level=logging.INFO)

    parser = argparse.ArgumentParser()
    parser.add_argument("--settings", required=True)
    parser.add_argument("--ipc-path", required=True)
    args = parser.parse_args()

    import django

    os.environ.setdefault("DJANGO_SETTINGS_MODULE", args.settings)
    django.setup()

    agent = Agent(Path(args.ipc_path))
    asyncio.run(agent.start())


if __name__ == "__main__":
    main()
