"""tako — Rust-core, Python-facade framework for enterprise agentic systems.

The thin Python facade re-exports a stable, ergonomic API on top of the
compiled Rust extension `tako._native`. End users should import from
``tako`` only — never from ``tako._native`` directly.
"""

from __future__ import annotations

from . import compat, mcp, orchestrator, providers, secrets, tracing
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
from .orchestrator import Conductor, SingleAgent

__all__ = [
    "Budget",
    "ChatRequest",
    "ChatResponse",
    "Client",
    "Conductor",
    "ContentPart",
    "Message",
    "Role",
    "SingleAgent",
    "ToolSchema",
    "Usage",
    "compat",
    "mcp",
    "orchestrator",
    "providers",
    "secrets",
    "tracing",
]

__version__ = "0.3.0"
