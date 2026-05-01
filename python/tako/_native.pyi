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
        stream: Any | None = ...,
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
        url_prefetch: bool = ...,
        url_prefetch_allow_http: bool = ...,
        url_prefetch_allow_private_ips: bool = ...,
        url_prefetch_allow_hosts: list[str] | None = ...,
        url_prefetch_allow_cidrs: list[str] | None = ...,
        url_prefetch_timeout_secs: int | None = ...,
        url_prefetch_max_bytes: int | None = ...,
    ) -> None: ...
    def id(self) -> str: ...

class Ollama:
    def __init__(
        self,
        model: str,
        base_url: str | None = ...,
        timeout_secs: int | None = ...,
        url_prefetch: bool = ...,
        url_prefetch_allow_http: bool = ...,
        url_prefetch_allow_private_ips: bool = ...,
        url_prefetch_allow_hosts: list[str] | None = ...,
        url_prefetch_allow_cidrs: list[str] | None = ...,
        url_prefetch_timeout_secs: int | None = ...,
        url_prefetch_max_bytes: int | None = ...,
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

class Vertex:
    def __init__(
        self,
        project_id: str,
        model: str,
        access_token: str,
        location: str | None = ...,
        endpoint_url: str | None = ...,
        timeout_secs: int | None = ...,
    ) -> None: ...
    def id(self) -> str: ...

class HttpGeneric:
    def __init__(
        self,
        id: str,
        model: str,
        url: str,
        body_template: Any,
        response_text_pointer: str,
        headers: list[tuple[str, str]] | None = ...,
        timeout_secs: int | None = ...,
        stream_config: dict[str, Any] | None = ...,
    ) -> None: ...
    def id(self) -> str: ...
    def supports_streaming(self) -> bool: ...

class VaultResolver:
    def __init__(self, addr: str, token: str) -> None: ...
    def resolve(self, key: str) -> Awaitable[str]: ...

class AzureKeyVaultResolver:
    def __init__(
        self,
        vault_url: str,
        access_token: str,
        api_version: str | None = ...,
    ) -> None: ...
    def resolve(self, key: str) -> Awaitable[str]: ...

class GcpSecretManagerResolver:
    def __init__(
        self,
        project_id: str,
        access_token: str,
        endpoint_url: str | None = ...,
    ) -> None: ...
    def resolve(self, key: str) -> Awaitable[str]: ...

class AwsSecretsManagerResolver:
    def __init__(
        self,
        region: str | None = ...,
        profile_name: str | None = ...,
        endpoint_url: str | None = ...,
    ) -> None: ...
    def resolve(self, key: str) -> Awaitable[str]: ...

class Stdio:
    def __init__(self, command: str, args: list[str] | None = ...) -> None: ...

class StreamableHttp:
    def __init__(
        self,
        url: str,
        headers: list[tuple[str, str]] | None = ...,
        timeout_secs: int | None = ...,
    ) -> None: ...

class WebSocket:
    """Available when the wheel is built with the ``ws`` feature."""

    def __init__(self, url: str) -> None: ...

class Grpc:
    """Available when the wheel is built with the ``grpc`` feature."""

    def __init__(
        self,
        endpoint: str,
        *,
        ca_pem: bytes | None = ...,
        client_cert_pem: bytes | None = ...,
        client_key_pem: bytes | None = ...,
        domain_name: str | None = ...,
    ) -> None: ...

class CatalogueVerifier:
    """Available when the wheel is built with the ``sigstore`` feature."""

    def __init__(self, pem: bytes) -> None: ...
    @staticmethod
    def from_pem_path(path: str) -> CatalogueVerifier: ...
    def verify(self, manifest: bytes, signature: bytes) -> tuple[str | None, str]: ...

class KeylessVerifier:
    """Available when the wheel is built with the ``sigstore`` feature."""

    def __init__(
        self,
        issuer: str,
        san: str,
        *,
        san_is_regex: bool = False,
        trust_root: TrustRoot | None = ...,
        rekor_public_key_pem: bytes | None = ...,
        rekor_min_tree_size: int | None = ...,
    ) -> None: ...
    def verify_bundle(self, manifest: bytes, bundle: bytes) -> tuple[str | None, str]: ...
    def verify_protobuf_bundle(
        self,
        manifest: bytes,
        protobuf_bundle: bytes,
    ) -> tuple[str | None, str]: ...
    def rekor_max_tree_size(self) -> int: ...

class TrustRoot:
    """Available when the wheel is built with the ``sigstore`` feature."""

    def __init__(
        self,
        roots_pem: bytes,
        intermediates_pem: bytes | None = ...,
    ) -> None: ...
    @staticmethod
    def from_paths(
        roots_path: str,
        intermediates_path: str | None = ...,
    ) -> TrustRoot: ...

class JsonStateStore:
    """Available when the wheel is built with the ``sigstore`` feature."""

    def __init__(self, path: str) -> None: ...
    def load(self) -> int: ...
    def save(self, n: int) -> None: ...
    def seed(self, verifier: KeylessVerifier) -> KeylessVerifier: ...
    def persist(self, verifier: KeylessVerifier) -> None: ...
    def path(self) -> str: ...

class RedisStateStore:
    """Available when the wheel is built with the ``redis`` feature."""

    @staticmethod
    def connect(url: str, key: str | None = ...) -> Awaitable[RedisStateStore]: ...
    def load(self) -> Awaitable[int]: ...
    def save(self, n: int) -> Awaitable[None]: ...
    def seed(self, verifier: KeylessVerifier) -> Awaitable[KeylessVerifier]: ...
    def persist(self, verifier: KeylessVerifier) -> Awaitable[None]: ...
    def key(self) -> str: ...

class InMemoryBudgetBackend:
    def __init__(self) -> None: ...
    def current_usage(self, tenant_id: str) -> Awaitable[tuple[float, int]]: ...
    def record(self, tenant_id: str, usd: float, tokens: int) -> Awaitable[None]: ...

class RedisBudgetBackend:
    """Available when the wheel is built with the ``redis`` feature."""

    def __init__(
        self,
        url: str,
        key_prefix: str | None = ...,
        ttl_secs: int | None = ...,
    ) -> None: ...
    def current_usage(self, tenant_id: str) -> Awaitable[tuple[float, int]]: ...
    def record(self, tenant_id: str, usd: float, tokens: int) -> Awaitable[None]: ...

class Orchestrator:
    def __init__(
        self,
        provider: Any,
        max_steps: int = ...,
        mcp_servers: list[Any] | None = ...,
        candidates: list[Any] | None = ...,
        router: Any | None = ...,
        budget: Any | None = ...,
        budget_backend: Any | None = ...,
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

class Trinity:
    def __init__(
        self,
        roles: list[tuple[str, Any]],
        router: Any,
        max_steps: int = ...,
        budget: Any | None = ...,
        budget_backend: Any | None = ...,
        verifier: Any | None = ...,
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

class RuleBasedVerifier:
    def __init__(
        self,
        min_chars: int = ...,
        pattern: str | None = ...,
    ) -> None: ...

class AbMcts:
    def __init__(
        self,
        provider: Any,
        verifier: Any,
        max_iterations: int = ...,
        branching_factor: int = ...,
        max_steps_per_rollout: int = ...,
        temperature: float = ...,
        min_confidence: float = ...,
        candidates: list[Any] | None = ...,
        router: Any | None = ...,
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
    def stream(
        self,
        prompt: str,
        tenant_id: str | None = ...,
        user_id: str | None = ...,
    ) -> Awaitable[OrchEventStream]: ...

class SelfCaller:
    def __init__(
        self,
        inner: Any,
        confidence: Any,
        max_depth: int = ...,
        min_confidence: float = ...,
        revision_prompt: str | None = ...,
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
    def stream(
        self,
        prompt: str,
        tenant_id: str | None = ...,
        user_id: str | None = ...,
    ) -> Awaitable[OrchEventStream]: ...

class OrchEvent:
    @property
    def kind(self) -> str: ...
    @property
    def step(self) -> int | None: ...
    @property
    def delta(self) -> str | None: ...
    @property
    def name(self) -> str | None: ...
    @property
    def id(self) -> str | None: ...
    @property
    def result(self) -> Any: ...
    @property
    def is_error(self) -> bool | None: ...
    @property
    def text(self) -> str | None: ...
    @property
    def usage(self) -> dict[str, int] | None: ...
    @property
    def branch(self) -> int | None: ...
    @property
    def score(self) -> float | None: ...
    @property
    def depth(self) -> int | None: ...
    @property
    def confidence(self) -> float | None: ...

class OrchEventStream:
    def __aiter__(self) -> OrchEventStream: ...
    def __anext__(self) -> Awaitable[OrchEvent]: ...

class RuleBasedGuard:
    def __init__(
        self,
        min_chars: int = ...,
        pattern: str | None = ...,
    ) -> None: ...

class LlmJudgeGuard:
    def __init__(
        self,
        judge: Any,
        rubric: str,
        budget: Any | None = ...,
        budget_backend: Any | None = ...,
        streaming_min_chars: int | None = ...,
        streaming_every_n: int | None = ...,
    ) -> None: ...

class RegexRouter:
    def __init__(self) -> None: ...

class OnnxRouter:
    def __init__(self, path: str) -> None: ...

class Conductor:
    def __init__(
        self,
        coordinator: Any,
        workers: dict[str, Any],
        max_steps: int = ...,
        max_fanout: int = ...,
        worker_timeout_secs: int = ...,
        fail_fast: bool = ...,
        budget: Any | None = ...,
        budget_backend: Any | None = ...,
        verifier: Any | None = ...,
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
def featurise_text(text: str) -> list[float]: ...
