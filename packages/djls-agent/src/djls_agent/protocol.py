from __future__ import annotations

import json
from abc import ABC
from abc import abstractmethod
from typing import Any

from ._typing import override
from .schema import Response


class MessageProtocol(ABC):
    @abstractmethod
    def serialize(self, response: Response) -> bytes:
        pass

    @abstractmethod
    def deserialize(self, data: bytes) -> Any:
        pass


class JsonProtocol(MessageProtocol):
    @override
    def serialize(self, response: Response, logger) -> bytes:
        logger.debug(
            f"response={response!r}"
        )  # Use !r to get a single-line representation
        data = response.model_dump(exclude_none=True, mode="json")
        logger.debug(f"data={data!r}")  # Use !r here too
        dump = json.dumps(data).encode()
        logger.debug(f"dump={dump!r}")  # And here
        return dump

    @override
    def deserialize(self, data: bytes) -> Any:
        return json.loads(data)
