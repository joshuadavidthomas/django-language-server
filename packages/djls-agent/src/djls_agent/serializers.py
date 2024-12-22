from __future__ import annotations

from abc import ABC
from abc import abstractmethod
from typing import Generic
from typing import TypeVar

from pydantic import BaseModel

from ._typing import override

T = TypeVar("T", bound=BaseModel)


class Serializer(ABC, Generic[T]):
    @abstractmethod
    def encode(self, message: BaseModel) -> bytes: ...

    @abstractmethod
    def decode(self, data: bytes, model_type: type[T]) -> T: ...


class JsonSerializer(Serializer[T]):
    @override
    def encode(self, message: BaseModel) -> bytes:
        return message.model_dump_json(exclude_none=True).encode()

    @override
    def decode(self, data: bytes, model_type: type[T]) -> T:
        return model_type.model_validate_json(data)
