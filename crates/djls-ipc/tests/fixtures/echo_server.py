from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path


async def handle_client(reader, writer):
    while True:
        try:
            data = await reader.readline()
            if not data:
                break

            # Parse the incoming message
            message = json.loads(data)
            # Echo back with same ID but just echo the content
            response = {"id": message["id"], "content": message["content"]}
            writer.write(json.dumps(response).encode() + b"\n")
            await writer.drain()
        except Exception:
            break
    writer.close()
    await writer.wait_closed()


async def main(ipc_path):
    try:
        Path(ipc_path).unlink()
    except FileNotFoundError:
        pass

    server = await asyncio.start_unix_server(
        handle_client,
        path=ipc_path,
    )

    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--ipc-path", required=True)
    args = parser.parse_args()
    asyncio.run(main(args.ipc_path))
