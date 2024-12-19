from __future__ import annotations

import asyncio
import struct
import sys
from typing import Optional
from typing import Type

from .logging import configure_logging
from .protocol import JsonProtocol
from .protocol import MessageProtocol
from .schema import ErrorResponse
from .schema import Messages
from .schema import Request
from .schema import Response

logger = configure_logging()


class LSPAgent:
    def __init__(self, protocol: Type[MessageProtocol]):
        from .handlers import handlers

        self.handlers = handlers
        self.protocol = protocol()

    async def serve(self):
        print("ready", flush=True)

        try:
            import django

            django.setup()
        except Exception as e:
            error_response = self.create_error("django_error", str(e))
            self.write_message(error_response)

        while True:
            try:
                message = self.read_message()
                logger.debug(f"read_message: {message=}")
                if not message:
                    break

                response = await self.handle_message(message)
                logger.debug(f"handle_message: {response=}")
                self.write_message(response)

            except Exception as e:
                error_response = self.create_error("unknown_error", str(e))
                self.write_message(error_response)

    def read_message(self) -> Optional[Request]:
        length_bytes = sys.stdin.buffer.read(4)
        logger.debug("Read length bytes: %r", length_bytes)
        if not length_bytes:
            return None

        length = struct.unpack(">I", length_bytes)[0]
        logger.debug("Unpacked length: %d", length)
        data = sys.stdin.buffer.read(length)
        logger.debug("Read data bytes: %r", data)

        message_data = self.protocol.deserialize(data)
        return Request.model_validate(message_data)

    async def handle_message(self, message: Request) -> Response:
        logger.debug("Message type: %s", message.message.value)

        handler = self.handlers.get(message.message)
        if not handler:
            logger.error("Unknown message type: %s", message.message.value)
            return self.create_error(
                "invalid_request",
                f"Unknown message type: {message.message.value}",
                message,
            )

        try:
            # Now handler is properly typed to return Response
            return await handler(message.message)
        except Exception as e:
            logger.exception("Error executing handler")
            return self.create_error("unknown_error", str(e))

    def write_message(self, message: Response) -> None:
        data = self.protocol.serialize(message, logger)
        length = struct.pack(">I", len(data))

        logger.debug(
            f"Writing length: {len(data)}, hex: {' '.join(f'{b:02x}' for b in length)}"
        )

        # Write length and flush immediately
        sys.stdout.buffer.write(length)
        sys.stdout.buffer.flush()

        logger.debug("Length written and flushed")

        # Write data and flush
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()

        logger.debug(
            f"Data written and flushed - total bytes: {len(length) + len(data)}"
        )

    def create_error(
        self, code: str, message: str, request: Request | None = None
    ) -> Response:
        return Response(
            data={},
            error=ErrorResponse(code=code, message=message),
            message=request.message.value if request else Messages.UNKNOWN,
            success=False,
        )


async def main() -> None:
    logger.debug("Starting djls-agent...")

    try:
        logger.debug("Initializing LSPAgent...")
        agent = LSPAgent(JsonProtocol)
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
    asyncio.run(main())
