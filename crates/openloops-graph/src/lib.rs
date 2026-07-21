#![forbid(unsafe_code)]

/// Phase 0 supplies no Graph transport, permission, or network path.
#[must_use]
pub const fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn graph_is_unavailable() {
        assert!(!super::is_available());
    }
}
