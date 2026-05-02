"""tako — Rust-core, Python-facade framework for enterprise agentic systems.

The thin Python facade re-exports a stable, ergonomic API on top of the
compiled Rust extension `tako._native`. End users should import from
``tako`` only — never from ``tako._native`` directly.
"""

from __future__ import annotations

from . import (
    budget,
    compat,
    guards,
    mcp,
    orchestrator,
    providers,
    routers,
    secrets,
    sigstore,
    tracing,
    verifiers,
)
from .budget import Budget
from .client import Client
from .models import (
    ChatRequest,
    ChatResponse,
    ContentPart,
    Message,
    Role,
    ToolSchema,
    Usage,
)
from .orchestrator import AbMcts, Conductor, SelfCaller, SingleAgent, Trinity

__all__ = [
    "AbMcts",
    "Budget",
    "ChatRequest",
    "ChatResponse",
    "Client",
    "Conductor",
    "ContentPart",
    "Message",
    "Role",
    "SelfCaller",
    "SingleAgent",
    "ToolSchema",
    "Trinity",
    "Usage",
    "budget",
    "compat",
    "guards",
    "mcp",
    "orchestrator",
    "providers",
    "routers",
    "secrets",
    "sigstore",
    "tracing",
    "verifiers",
]

__version__ = "0.36.0"
