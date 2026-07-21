#![forbid(unsafe_code)]

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
