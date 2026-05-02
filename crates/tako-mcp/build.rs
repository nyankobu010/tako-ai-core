//! Codegen for the `grpc` Cargo feature.
//!
//! When the `grpc` feature is enabled, run `tonic-prost-build` over
//! `proto/mcp_bridge.proto`. When it isn't, this script is a no-op so
//! `cargo build -p tako-mcp` (and the workspace default build) doesn't
//! require `protoc` on PATH.
#![allow(unsafe_code)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/mcp_bridge.proto");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var_os("CARGO_FEATURE_GRPC").is_none() {
        return Ok(());
    }

    // Point `prost-build` at the bundled `protoc` so contributors don't
    // need a system-wide install to build with `--features grpc`. Honour
    // an explicit `PROTOC` env override if set.
    if std::env::var_os("PROTOC").is_none() {
        // Safety: build scripts run single-threaded; Rust 2024 still
        // requires `unsafe` for `set_var` to surface this contract.
        unsafe {
            std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
        }
    }

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&["proto/mcp_bridge.proto"], &["proto"])?;
    Ok(())
}
