#![forbid(unsafe_code)]

//! `openloops-inference`: the ADR-006 byte-deterministic content
//! canonicalizer.
//!
//! This crate turns a Graph-shaped message body (HTML or plain text) into
//! the transient canonical block representation whose byte behavior the
//! ADR-006 evidence anchors depend on, and the bounded
//! [`message::CanonicalMessage`] projection a (still-disabled) model
//! provider payload would use. Every function here is pure: no I/O, no
//! clock, no network, no global state, and every error is typed and
//! content-free (see [`error`]).
//!
//! [`is_available`] answers a narrower, separate question: whether Phase 0
//! has an active model-provider/content-analysis path. It does not, so it
//! still returns `false` — `contracts/model/provider-boundary.json`
//! `default_provider` is `"disabled"` and every profile is
//! `disabled_pending_gates`/`optional_disabled_pending_adapter_gates`. The
//! canonicalizer in this crate has no such gate: it is a deterministic
//! transform with no provider, network, or persistence dependency, so it
//! is implemented ahead of the (still-inactive) provider path it feeds.

pub mod blocks;
pub mod canonical;
pub mod error;
pub mod message;
pub mod validation;
pub mod walker;

/// Phase 0 supplies no model provider or content-analysis path.
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
