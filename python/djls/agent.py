from __future__ import annotations

import struct
import sys
from typing import Any
from typing import cast

from google.protobuf.message import Message

from .logging import configure_logging
from .proto.v1 import messages_pb2

logger = configure_logging()


class LSPAgent:
    async def serve(self):
        print("ready", flush=True)

        try:
            import django

            django.setup()
        except Exception as e:
            error_response = self.create_error(messages_pb2.Error.DJANGO_ERROR, str(e))
            self.write_message(error_response)

        while True:
            try:
                data = self.read_message()
                if not data:
                    break

                response = await self.handle_request(data)
                self.write_message(response)

            except Exception as e:
                error_response = self.create_error(messages_pb2.Error.UNKNOWN, str(e))
                self.write_message(error_response)

    def read_message(self) -> bytes | None:
        length_bytes = sys.stdin.buffer.read(4)
        logger.debug("Read length bytes: %r", length_bytes)
        if not length_bytes:
            return None

        length = struct.unpack(">I", length_bytes)[0]
        logger.debug("Unpacked length: %d", length)
        data = sys.stdin.buffer.read(length)
        logger.debug("Read data bytes: %r", data)
        return data

    async def handle_request(self, request_data: bytes) -> Message:
        from .handlers import handlers

        request = messages_pb2.Request()
        request.ParseFromString(request_data)

        command_name = request.WhichOneof("command")
        logger.debug("Command name: %s", command_name)

        if not command_name:
            logger.error("No command specified")
            return self.create_error(
                messages_pb2.Error.INVALID_REQUEST, "No command specified"
            )

        handler = handlers.get(command_name)
        if not handler:
            logger.error("Unknown command: %s", command_name)
            return self.create_error(
                messages_pb2.Error.INVALID_REQUEST, f"Unknown command: {command_name}"
            )

        try:
            command_message = getattr(request, command_name)
            result = await handler(command_message)
            return messages_pb2.Response(**{command_name: cast(Any, result)})
        except Exception as e:
            logger.exception("Error executing command")
            return self.create_error(messages_pb2.Error.UNKNOWN, str(e))

    def write_message(self, message: Message) -> None:
        data = message.SerializeToString()
        logger.debug(f"Sending response, length: {len(data)}, data: {data!r}")
        length = struct.pack(">I", len(data))
        logger.debug(f"Length bytes: {length!r}")
        sys.stdout.buffer.write(length)
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()

    def create_error(
        self, code: messages_pb2.Error.Code, message: str
    ) -> messages_pb2.Response:
        response = messages_pb2.Response()
        response.error.code = code
        response.error.message = message
        return response


async def main() -> None:
    logger.debug("Starting DJLS...")

    try:
        logger.debug("Initializing LSPAgent...")
        agent = LSPAgent()
        logger.debug("Starting LSPAgent serve...")
        await agent.serve()
    except KeyboardInterrupt:
        logger.debug("Received KeyboardInterrupt")
        sys.exit(0)
    except Exception as e:
        logger.exception("Fatal error")
        print(f"error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    import asyncio

    asyncio.run(main())
