"""Pydantic v2 mirrors of the core `tako_core` types.

These are the canonical Python representations users construct in code.
The native extension accepts both Pydantic models and dicts; results that
come back from `tako._native` are converted to plain Python types
(strings, lists, dicts) and can be re-validated through these models.
"""

from __future__ import annotations

from enum import Enum
from typing import Any

from pydantic import BaseModel, ConfigDict, Field


class Role(str, Enum):
    SYSTEM = "system"
    USER = "user"
    ASSISTANT = "assistant"
    TOOL = "tool"


class ContentPart(BaseModel):
    """A typed content block: text, image, tool_call, or tool_result."""

    model_config = ConfigDict(extra="allow")

    type: str
    text: str | None = None
    mime: str | None = None
    data_b64: str | None = None
    id: str | None = None
    name: str | None = None
    args: dict[str, Any] | None = None
    result: Any | None = None
    is_error: bool = False


class Message(BaseModel):
    role: Role
    content: list[ContentPart] = Field(default_factory=list)

    @classmethod
    def system(cls, text: str) -> Message:
        return cls(role=Role.SYSTEM, content=[ContentPart(type="text", text=text)])

    @classmethod
    def user(cls, text: str) -> Message:
        return cls(role=Role.USER, content=[ContentPart(type="text", text=text)])

    @classmethod
    def assistant(cls, text: str) -> Message:
        return cls(role=Role.ASSISTANT, content=[ContentPart(type="text", text=text)])


class ToolSchema(BaseModel):
    name: str
    description: str
    input_schema: dict[str, Any]


class Usage(BaseModel):
    input_tokens: int = 0
    output_tokens: int = 0

    @property
    def total(self) -> int:
        return self.input_tokens + self.output_tokens


class ChatRequest(BaseModel):
    model: str
    messages: list[Message]
    tools: list[ToolSchema] = Field(default_factory=list)
    temperature: float | None = None
    max_tokens: int | None = None
    stop: list[str] = Field(default_factory=list)
    stream: bool = False
    metadata: dict[str, str] = Field(default_factory=dict)


class ChatResponse(BaseModel):
    message: Message
    finish_reason: str
    usage: Usage = Field(default_factory=Usage)
