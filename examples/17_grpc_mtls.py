"""Phase 5.B: connect to an MCP gRPC bridge with mutual TLS.

Demonstrates the new TLS kwargs on :class:`tako.mcp.Grpc`. The example
itself doesn't dial a real server (that requires PKI material an
operator would already own); instead it shows the call shape and walks
through the validation rules.

Run with a wheel built using the ``grpc`` feature::

    maturin develop --release --features grpc
    python examples/17_grpc_mtls.py
"""

from __future__ import annotations

import tako


def main() -> None:
    # In production: read these from your operator-managed PKI.
    ca_pem = b"-----BEGIN CERTIFICATE-----\n... CA cert ...\n-----END CERTIFICATE-----\n"
    client_cert_pem = (
        b"-----BEGIN CERTIFICATE-----\n... client cert ...\n-----END CERTIFICATE-----\n"
    )

    print("Call shape:")
    print(
        "  tako.mcp.Grpc(\n"
        "      'https://mcp.example.com:50051',\n"
        "      ca_pem=ca_pem,\n"
        "      client_cert_pem=client_cert_pem,\n"
        "      client_key_pem=client_key_pem,\n"
        "      domain_name='mcp.example.com',\n"
        "  )"
    )
    print()
    print("Path form (read from disk eagerly):")
    print(
        "  tako.mcp.Grpc(\n"
        "      'https://mcp.example.com:50051',\n"
        "      ca_path='/etc/tako/ca.pem',\n"
        "      client_cert_path='/etc/tako/client.pem',\n"
        "      client_key_path='/etc/tako/client.key',\n"
        "  )"
    )
    print()
    print("Validation: half-pair client identity raises eagerly.")
    try:
        tako.mcp.Grpc(
            "https://example.invalid:1",
            ca_pem=ca_pem,
            client_cert_pem=client_cert_pem,
            # client_key_pem omitted on purpose
        )
    except ValueError as e:
        print(f"  -> {e}")


if __name__ == "__main__":
    main()
