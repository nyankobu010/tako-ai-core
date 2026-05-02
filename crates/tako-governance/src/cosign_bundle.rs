//! Minimal hand-rolled subset of the Sigstore protobuf-specs `Bundle`
//! v1 message (Phase 7.C).
//!
//! Exists so [`super::sigstore::KeylessBundle::from_protobuf_bundle`]
//! can decode the output of `cosign sign-blob --bundle out.pb`
//! without pulling the full `sigstore-protobuf-specs` crate (and its
//! transitive deps) into the dep tree.
//!
//! Field tags follow the upstream
//! [`sigstore_bundle.proto`](https://github.com/sigstore/protobuf-specs/blob/main/protos/sigstore_bundle.proto)
//! and [`sigstore_rekor.proto`](https://github.com/sigstore/protobuf-specs/blob/main/protos/sigstore_rekor.proto).
//! Fields tako does not consume — `kind_version`, `checkpoint`,
//! timestamp-verification material, DSSE envelopes, public-key
//! verifiers — are intentionally omitted; prost ignores unknown tags
//! during decode, so cosign-emitted bundles that include those fields
//! still parse cleanly.

#![allow(missing_docs)]

use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct Bundle {
    #[prost(string, tag = "1")]
    pub media_type: String,
    #[prost(message, optional, tag = "2")]
    pub verification_material: Option<VerificationMaterial>,
    /// `oneof verifier { MessageSignature message_signature = 3; ... }`
    /// — only the `message_signature` arm is consumed by the tako
    /// adapter (cosign blob signing always picks this).
    #[prost(message, optional, tag = "3")]
    pub message_signature: Option<MessageSignature>,
}

#[derive(Clone, PartialEq, Message)]
pub struct VerificationMaterial {
    /// `oneof content { PublicKey public_key = 1;
    /// X509CertificateChain x509_certificate_chain = 2;
    /// Certificate certificate = 5; }` — we read whichever variant
    /// is present and surface a chain-of-one if `certificate` is set.
    #[prost(message, optional, tag = "2")]
    pub x509_certificate_chain: Option<X509CertificateChain>,
    #[prost(message, optional, tag = "5")]
    pub certificate: Option<X509Certificate>,
    #[prost(message, repeated, tag = "3")]
    pub tlog_entries: Vec<TransparencyLogEntry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct X509CertificateChain {
    #[prost(message, repeated, tag = "1")]
    pub certificates: Vec<X509Certificate>,
}

#[derive(Clone, PartialEq, Message)]
pub struct X509Certificate {
    #[prost(bytes = "vec", tag = "1")]
    pub raw_bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct MessageSignature {
    #[prost(message, optional, tag = "1")]
    pub message_digest: Option<HashOutput>,
    #[prost(bytes = "vec", tag = "2")]
    pub signature: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct HashOutput {
    #[prost(int32, tag = "1")]
    pub algorithm: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub digest: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct TransparencyLogEntry {
    #[prost(int64, tag = "1")]
    pub log_index: i64,
    #[prost(message, optional, tag = "2")]
    pub log_id: Option<LogId>,
    #[prost(int64, tag = "4")]
    pub integrated_time: i64,
    #[prost(message, optional, tag = "5")]
    pub inclusion_promise: Option<InclusionPromise>,
    #[prost(message, optional, tag = "6")]
    pub inclusion_proof: Option<InclusionProof>,
    #[prost(bytes = "vec", tag = "7")]
    pub canonicalized_body: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct LogId {
    #[prost(bytes = "vec", tag = "1")]
    pub key_id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct InclusionPromise {
    #[prost(bytes = "vec", tag = "1")]
    pub signed_entry_timestamp: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct InclusionProof {
    #[prost(int64, tag = "1")]
    pub log_index: i64,
    #[prost(bytes = "vec", tag = "2")]
    pub root_hash: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub tree_size: i64,
    #[prost(bytes = "vec", repeated, tag = "4")]
    pub hashes: Vec<Vec<u8>>,
}

/// Decode a serialised cosign `Bundle` proto.
pub fn decode(bytes: &[u8]) -> Result<Bundle, prost::DecodeError> {
    Bundle::decode(bytes)
}
