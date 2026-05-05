"""Phase 7.C — `tako.sigstore.KeylessVerifier.verify_protobuf_bundle`
Python-facade smoke tests.

The happy-path round-trip (programmatically-built cosign `Bundle` proto
→ JSON shape → identity / signature / chain / Rekor pipeline) lives in
the Rust side at `crates/tako-governance/src/sigstore.rs::protobuf_tests`.
These tests exercise the FFI shim: the method shows up on the
verifier when the wheel was built with `sigstore-protobuf`, and bogus
bytes raise a clear `ValueError`.

Auto-skipped when the wheel was built without the feature.
"""

from __future__ import annotations

import pytest
import tako
from tako import _native

# `tako.sigstore` is a regular Python module that always imports — the
# `sigstore` feature only gates the native pyclasses. Skip the module
# unless `_native.KeylessVerifier` actually got compiled in.
if not hasattr(_native, "KeylessVerifier"):
    pytest.skip("wheel built without --features sigstore", allow_module_level=True)


def _make_verifier() -> tako.sigstore.KeylessVerifier:
    return tako.sigstore.KeylessVerifier(
        "https://token.actions.githubusercontent.com",
        "https://example.com/svc",
    )


def _has_protobuf() -> bool:
    v = _make_verifier()
    return hasattr(v._native, "verify_protobuf_bundle")


def test_verify_protobuf_bundle_method_present_when_feature_on() -> None:
    if not _has_protobuf():
        pytest.skip("wheel built without `sigstore-protobuf` feature")
    v = _make_verifier()
    assert hasattr(v, "verify_protobuf_bundle")


def test_verify_protobuf_bundle_rejects_garbage_bytes() -> None:
    if not _has_protobuf():
        pytest.skip("wheel built without `sigstore-protobuf` feature")
    v = _make_verifier()
    # Random non-protobuf bytes — prost decode must reject.
    with pytest.raises(ValueError) as exc:
        v.verify_protobuf_bundle(b"manifest-doesnt-matter", b"\xff\xfe\xfd\xfc not-a-protobuf")
    assert "protobuf bundle" in str(exc.value)


def test_verify_protobuf_bundle_attribute_error_when_feature_off() -> None:
    if _has_protobuf():
        pytest.skip("wheel built with `sigstore-protobuf` feature")
    v = _make_verifier()
    with pytest.raises(AttributeError) as exc:
        v.verify_protobuf_bundle(b"m", b"b")
    assert "sigstore-protobuf" in str(exc.value)
