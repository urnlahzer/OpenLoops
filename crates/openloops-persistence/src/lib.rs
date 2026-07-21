#![forbid(unsafe_code)]

/// Phase 0 supplies no durable store and writes no runtime values.
#[must_use]
pub const fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn persistence_is_unavailable() {
        assert!(!super::is_available());
    }
}
