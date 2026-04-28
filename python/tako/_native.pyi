"""Type stubs for the compiled Rust extension `tako._native`.

Hand-written; keep in sync with `crates/tako-py/src/lib.rs` and the
`#[pyclass]` definitions in the surrounding modules.
"""

from typing import Any, Awaitable

__version__: str

class OpenAI:
    def __init__(
        self,
        model: str,
        api_key: str,
        base_url: str | None = ...,
        timeout_secs: int | None = ...,
        organization: str | None = ...,
    ) -> None: ...
    def id(self) -> str: ...

class Anthropic:
    def __init__(
        self,
        model: str,
        api_key: str,
        base_url: str | None = ...,
        timeout_secs: int | None = ...,
        default_max_tokens: int | None = ...,
    ) -> None: ...
    def id(self) -> str: ...

class FakeProvider:
    def __init__(
        self,
        canned_text: str = ...,
        id: str = ...,
        delay_ms: int = ...,
    ) -> None: ...
    def id(self) -> str: ...
    def call_count(self) -> int: ...

class Orchestrator:
    def __init__(self, provider: Any, max_steps: int = ...) -> None: ...
    def run(
        self,
        prompt: str,
        tenant_id: str | None = ...,
        user_id: str | None = ...,
    ) -> Awaitable[str]: ...
    def run_sync(
        self,
        prompt: str,
        tenant_id: str | None = ...,
        user_id: str | None = ...,
    ) -> str: ...

class Budget:
    def __init__(
        self,
        max_usd_per_request: float | None = ...,
        max_usd_per_day: float | None = ...,
        max_tokens_per_request: int | None = ...,
        max_usd_per_tenant_per_day: dict[str, float] | None = ...,
    ) -> None: ...

def init_tracing_py(filter: str | None = ..., json: bool = ...) -> None: ...
