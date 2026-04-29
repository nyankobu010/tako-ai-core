"""Type stubs for the compiled Rust extension `tako._native`.

Hand-written; keep in sync with `crates/tako-py/src/lib.rs` and the
`#[pyclass]` definitions in the surrounding modules.
"""

from collections.abc import Awaitable
from typing import Any

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

class PythonProvider:
    def __init__(
        self,
        id: str,
        chat: Any,
        max_context_tokens: int | None = ...,
    ) -> None: ...
    def id(self) -> str: ...

class Bedrock:
    def __init__(
        self,
        model: str,
        region: str | None = ...,
        endpoint_url: str | None = ...,
        profile_name: str | None = ...,
    ) -> None: ...
    def id(self) -> str: ...

class AzureOpenAi:
    def __init__(
        self,
        endpoint: str,
        deployment: str,
        api_key: str,
        api_version: str | None = ...,
        timeout_secs: int | None = ...,
    ) -> None: ...
    def id(self) -> str: ...

class Stdio:
    def __init__(self, command: str, args: list[str] | None = ...) -> None: ...

class StreamableHttp:
    def __init__(
        self,
        url: str,
        headers: list[tuple[str, str]] | None = ...,
        timeout_secs: int | None = ...,
    ) -> None: ...

class Orchestrator:
    def __init__(
        self,
        provider: Any,
        max_steps: int = ...,
        mcp_servers: list[Any] | None = ...,
    ) -> None: ...
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

class Conductor:
    def __init__(
        self,
        coordinator: Any,
        workers: dict[str, Any],
        max_steps: int = ...,
        max_fanout: int = ...,
        worker_timeout_secs: int = ...,
        fail_fast: bool = ...,
    ) -> None: ...
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
def init_otlp_tracing_py(
    endpoint: str,
    filter: str | None = ...,
    json: bool = ...,
) -> None: ...
def shutdown_otlp_py() -> None: ...
def serve_openai_py(
    orch: Any,
    host: str = ...,
    port: int = ...,
    tokens: dict[str, tuple[str, str]] | None = ...,
    models: list[str] | None = ...,
) -> str: ...
def shutdown_compat_py() -> None: ...
