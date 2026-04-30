"""Provider adapters: thin wrappers around the native classes.

Each adapter exposes a stable, kwargs-friendly Python constructor and
forwards to the underlying Rust builder via ``tako._native``.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from typing import Any

from tako import _native


class _ProviderBase:
    """Common attributes; a subclass of this is what callers receive back
    from each provider constructor. The ``_handle`` attribute carries the
    native object that ``Orchestrator`` accepts."""

    _handle: Any

    @property
    def id(self) -> str:
        return str(self._handle.id())


class OpenAI(_ProviderBase):
    """OpenAI chat.completions provider."""

    def __init__(
        self,
        model: str,
        api_key: str,
        *,
        base_url: str | None = None,
        timeout_secs: int | None = None,
        organization: str | None = None,
    ) -> None:
        self._handle = _native.OpenAI(
            model,
            api_key,
            base_url=base_url,
            timeout_secs=timeout_secs,
            organization=organization,
        )


class Anthropic(_ProviderBase):
    """Anthropic Messages API provider."""

    def __init__(
        self,
        model: str,
        api_key: str,
        *,
        base_url: str | None = None,
        timeout_secs: int | None = None,
        default_max_tokens: int | None = None,
    ) -> None:
        self._handle = _native.Anthropic(
            model,
            api_key,
            base_url=base_url,
            timeout_secs=timeout_secs,
            default_max_tokens=default_max_tokens,
        )


class Fake(_ProviderBase):
    """In-process fake provider for tests. Returns canned text and tracks call count."""

    def __init__(
        self,
        canned_text: str = "ok",
        *,
        id: str = "fake:test",
        delay_ms: int = 0,
    ) -> None:
        self._handle = _native.FakeProvider(canned_text, id, delay_ms)

    @property
    def call_count(self) -> int:
        return int(self._handle.call_count())


class AzureOpenAI(_ProviderBase):
    """Azure OpenAI provider.

    Wire format is identical to OpenAI's chat.completions; the routing layer
    differs: requests go to ``{endpoint}/openai/deployments/{deployment}/chat/completions``
    with an ``api-key`` header. ``deployment`` is the Azure deployment name
    (a user-defined alias mapping to a model — distinct from the underlying
    model id).
    """

    def __init__(
        self,
        endpoint: str,
        deployment: str,
        api_key: str,
        *,
        api_version: str | None = None,
        timeout_secs: int | None = None,
    ) -> None:
        self._handle = _native.AzureOpenAi(
            endpoint,
            deployment,
            api_key,
            api_version=api_version,
            timeout_secs=timeout_secs,
        )


class Vertex(_ProviderBase):
    """Google Vertex AI (Gemini) provider.

    Auth is intentionally deferred: pass a pre-resolved OAuth2 access token
    (or ``"$ENV:VAR"``). The provider does not refresh tokens — for long-
    lived processes, wire your own credential source (e.g. ``gcloud auth
    print-access-token``, the GKE metadata server, a service account JWT
    exchange) and rebuild the provider before tokens expire.

    Example::

        provider = tako.providers.Vertex(
            project_id="my-gcp-project",
            model="gemini-2.0-pro",
            access_token=os.environ["VERTEX_ACCESS_TOKEN"],
            location="us-central1",
        )
    """

    def __init__(
        self,
        project_id: str,
        model: str,
        access_token: str,
        *,
        location: str | None = None,
        endpoint_url: str | None = None,
        timeout_secs: int | None = None,
    ) -> None:
        self._handle = _native.Vertex(
            project_id,
            model,
            access_token,
            location=location,
            endpoint_url=endpoint_url,
            timeout_secs=timeout_secs,
        )


class Bedrock(_ProviderBase):
    """Amazon Bedrock provider via Converse + ConverseStream.

    Credentials come from the AWS default credential chain (env, profile,
    IRSA, IMDS) — pass ``profile_name`` to pin a specific named profile,
    or ``endpoint_url`` to talk to a VPC-private endpoint or local mock.
    """

    def __init__(
        self,
        model: str,
        *,
        region: str | None = None,
        endpoint_url: str | None = None,
        profile_name: str | None = None,
    ) -> None:
        self._handle = _native.Bedrock(
            model,
            region=region,
            endpoint_url=endpoint_url,
            profile_name=profile_name,
        )


class HttpGeneric(_ProviderBase):
    """Generic HTTP / SSE provider — point at any chat-completions-compatible endpoint.

    ``body_template`` is a JSON document where the literal strings
    ``"{{ request }}"``, ``"{{ model }}"``, ``"{{ messages }}"`` are replaced
    with the corresponding fields of the outgoing request at call time.
    ``response_text_pointer`` is a JSON Pointer (RFC 6901) into the response
    body that yields the assistant text.

    Pass ``stream_config={"kind": "openai_sse", ...}`` or
    ``{"kind": "ndjson", ...}`` to enable streaming;
    ``Capabilities.supports_streaming`` flips automatically. Header values
    may carry ``"$VAR_NAME"`` literals; the provider resolves those from
    the environment at construction.

    Example::

        provider = tako.providers.HttpGeneric(
            id="custom",
            model="my-model-v1",
            url="https://api.example.com/v1/chat/completions",
            body_template={"model": "{{ model }}", "messages": "{{ messages }}"},
            response_text_pointer="/choices/0/message/content",
            headers=[("Authorization", "Bearer $MY_API_KEY")],
            stream_config={"kind": "openai_sse"},
        )
    """

    def __init__(
        self,
        id: str,
        model: str,
        url: str,
        body_template: dict[str, Any] | list[Any] | str | int | float | bool | None,
        response_text_pointer: str,
        *,
        headers: list[tuple[str, str]] | None = None,
        timeout_secs: int | None = None,
        stream_config: dict[str, Any] | None = None,
    ) -> None:
        self._handle = _native.HttpGeneric(
            id,
            model,
            url,
            body_template,
            response_text_pointer,
            headers=headers,
            timeout_secs=timeout_secs,
            stream_config=stream_config,
        )

    @property
    def supports_streaming(self) -> bool:
        return bool(self._handle.supports_streaming())


# `chat` callables receive a request dict (model, messages, tools, ...) and
# return either a string (assistant text) or a dict
# {"text": str, "input_tokens"?: int, "output_tokens"?: int}.
PythonChat = Callable[[dict[str, Any]], Awaitable[Any]]
# Phase 10.D — `stream` callables receive the same request dict and return
# an async iterator of dicts (chat chunks). Each dict matches the
# `tako_core::ChatChunk` `kind`-tagged schema:
# `{"kind": "delta", "text": "..."}` or
# `{"kind": "end", "finish_reason": "stop", "usage": {"input_tokens": int,
# "output_tokens": int}}`.
PythonStream = Callable[[dict[str, Any]], Any]


class PythonProvider(_ProviderBase):
    """LlmProvider whose ``chat()`` is a Python async callable.

    Useful for prototyping vendor adapters in pure Python or wiring up a
    provider whose Rust crate doesn't exist yet.

    .. note::
       ``SingleAgent.run_sync()`` is not supported with a PythonProvider
       (the synchronous code path doesn't run a Python event loop, which
       the user's async ``chat`` callable needs). Always use the async
       ``run()`` API.

    Phase 10.D — pass ``stream=async_gen_fn`` to enable streaming. The
    callable is ``async def stream(request: dict) -> AsyncIterator[dict]``;
    yielded dicts match :class:`tako_core::ChatChunk`'s ``kind``-tagged
    schema. When ``stream=`` is set, the provider's
    ``supports_streaming`` capability flips to ``True`` so orchestrators
    that prefer streaming (Trinity, AB-MCTS) route through the streaming
    path automatically.

    Example::

        async def my_chat(request: dict) -> str:
            return f"echo: {request['messages'][-1]['content'][0]['text']}"

        async def my_stream(request: dict):
            yield {"kind": "delta", "text": "echo: "}
            yield {"kind": "delta", "text": request["messages"][-1]["content"][0]["text"]}
            yield {"kind": "end", "finish_reason": "stop",
                   "usage": {"input_tokens": 0, "output_tokens": 0}}

        provider = tako.providers.PythonProvider(
            "custom:echo", chat=my_chat, stream=my_stream,
        )
        agent = tako.SingleAgent(provider=provider)
    """

    def __init__(
        self,
        id: str,
        chat: PythonChat,
        *,
        stream: PythonStream | None = None,
        max_context_tokens: int | None = None,
    ) -> None:
        self._handle = _native.PythonProvider(
            id,
            chat,
            stream=stream,
            max_context_tokens=max_context_tokens,
        )
