"""Phase 10.A — JsonStateStore Python facade smoke test.

The full seed → verify → persist round-trip against a live keyless
bundle is exercised by the Rust integration test
``crates/tako-governance/tests/sigstore.rs::state_store_seed_persist``.
This Python-side test focuses on the kwargs and the file-only
load/save semantics that don't require a real signing fixture.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from tako import _native

# `tako.sigstore` is a regular Python module that always imports — the
# `sigstore` feature only gates the native pyclasses it wraps. Skip the
# module unless `_native.JsonStateStore` (and `KeylessVerifier`) actually
# got compiled in.
if not hasattr(_native, "JsonStateStore") or not hasattr(_native, "KeylessVerifier"):
    pytest.skip(
        "wheel built without --features sigstore; skipping JsonStateStore tests",
        allow_module_level=True,
    )

from tako.sigstore import JsonStateStore, KeylessVerifier


def test_load_against_missing_file_returns_zero(tmp_path: Path) -> None:
    store = JsonStateStore(str(tmp_path / "missing.json"))
    assert store.load() == 0


def test_save_then_load_round_trip(tmp_path: Path) -> None:
    store = JsonStateStore(str(tmp_path / "anchor.json"))
    store.save(42)
    assert store.load() == 42

    # Schema check: the file is the documented `rekor_min_tree_size`
    # JSON shape, not an opaque blob. Phase 11.A added a `version: 1`
    # field for forward-incompat detection; both fields are present
    # in fresh saves.
    raw = json.loads((tmp_path / "anchor.json").read_text())
    assert raw == {"rekor_min_tree_size": 42, "version": 1}


def test_seed_applies_persisted_value_to_verifier(tmp_path: Path) -> None:
    store = JsonStateStore(str(tmp_path / "anchor.json"))
    store.save(8)

    verifier = KeylessVerifier(
        issuer="https://accounts.example.com",
        san="ci@example.com",
    )
    assert verifier.rekor_max_tree_size() == 0

    returned = store.seed(verifier)
    assert returned is verifier
    assert verifier.rekor_max_tree_size() == 8


def test_persist_writes_back_high_water_mark(tmp_path: Path) -> None:
    store = JsonStateStore(str(tmp_path / "anchor.json"))
    verifier = KeylessVerifier(
        issuer="https://accounts.example.com",
        san="ci@example.com",
        rekor_min_tree_size=11,
    )
    store.persist(verifier)
    assert store.load() == 11


def test_path_property_reports_backing_file(tmp_path: Path) -> None:
    target = tmp_path / "anchor.json"
    store = JsonStateStore(str(target))
    assert store.path == str(target)
