#![forbid(unsafe_code)]

//! `openloops-inference`: byte-deterministic canonicalization and the
//! ADR-007 governed claim-analysis path.
//!
//! This crate turns a Graph-shaped message body (HTML or plain text) into
//! the transient canonical block representation whose byte behavior the
//! ADR-006 evidence anchors depend on, and the bounded
//! [`message::CanonicalMessage`] projection the governed model path uses.
//! Canonicalization and validation are pure; feature-gated providers perform
//! explicitly requested network calls. Errors remain typed and content-free
//! (see [`error`]).
//!
//! [`is_available`] answers the narrower compile-time question retained by
//! the core crate; provider availability is selected and reported by the
//! desktop application.

pub mod blocks;
pub mod canonical;
pub mod error;
pub mod message;
pub mod reply_history;
pub mod validation;
pub mod walker;

#[cfg(any(feature = "ollama-cloud", feature = "openrouter"))]
pub mod provider;

#[cfg(any(feature = "ollama-cloud", feature = "openrouter"))]
pub mod analysis;

#[cfg(feature = "ollama-cloud")]
pub mod ollama;

#[cfg(feature = "openrouter")]
pub mod openrouter;

/// The core crate does not choose or activate a model provider by itself.
#[must_use]
pub const fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn inference_is_unavailable() {
        assert!(!super::is_available());
    }
}
