//! Safe current-user DPAPI protect/unprotect with bounded sizes.
//!
//! This is the only caller of [`crate::dpapi_ffi`]; every other module in
//! this crate reaches DPAPI through here. `os_protection.blob_validation`
//! requires validating "exact key-bundle magic version length unique key IDs
//! algorithms statuses and bounded entries before use" after every unprotect
//! — this module enforces the generic size bound; the bundle-specific magic
//! and field checks live in [`crate::state_root`], which is the one type
//! this crate protects today.

use crate::dpapi_ffi::{DpapiFfiError, protect as ffi_protect, unprotect as ffi_unprotect};

/// A rejected protect/unprotect call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DpapiError {
    /// The plaintext or blob was empty or exceeded the caller's bound.
    SizeOutOfBounds,
    /// The underlying OS call failed; see [`DpapiFfiError`] for the
    /// (already-sanitized) reason class.
    Os(DpapiFfiError),
}

/// Protects `plaintext` (current user, `CRYPTPROTECT_UI_FORBIDDEN`, no
/// description, no optional entropy, never `CRYPTPROTECT_LOCAL_MACHINE`),
/// rejecting anything outside `1..=max_len` bytes before the OS call.
///
/// # Errors
///
/// Returns [`DpapiError::SizeOutOfBounds`] or [`DpapiError::Os`].
pub fn protect_bounded(
    plaintext: &[u8],
    max_len: usize,
) -> Result<zeroize::Zeroizing<Vec<u8>>, DpapiError> {
    if plaintext.is_empty() || plaintext.len() > max_len {
        return Err(DpapiError::SizeOutOfBounds);
    }
    ffi_protect(plaintext).map_err(DpapiError::Os)
}

/// Unprotects `blob`, rejecting anything outside `1..=max_len` bytes before
/// the OS call and re-checking the recovered plaintext against `max_len`
/// after, so a corrupted or hostile blob can never yield an unbounded
/// allocation downstream.
///
/// # Errors
///
/// Returns [`DpapiError::SizeOutOfBounds`] or [`DpapiError::Os`].
pub fn unprotect_bounded(
    blob: &[u8],
    max_len: usize,
) -> Result<zeroize::Zeroizing<Vec<u8>>, DpapiError> {
    if blob.is_empty() || blob.len() > max_len {
        return Err(DpapiError::SizeOutOfBounds);
    }
    let plaintext = ffi_unprotect(blob).map_err(DpapiError::Os)?;
    if plaintext.len() > max_len {
        return Err(DpapiError::SizeOutOfBounds);
    }
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::{DpapiError, protect_bounded, unprotect_bounded};

    /// DPAPI adds a fixed metadata overhead to the ciphertext it returns
    /// (typically well over 64 bytes even for a tiny plaintext), so the
    /// bound used to unprotect a blob is necessarily larger than the bound
    /// used to protect the plaintext that produced it; the two bounds are
    /// intentionally independent parameters of this module's API.
    const GENEROUS_PROTECTED_BOUND: usize = 4096;

    #[test]
    fn round_trips_within_bound() {
        let protected = protect_bounded(b"synthetic", 64).unwrap();
        let recovered = unprotect_bounded(&protected, GENEROUS_PROTECTED_BOUND).unwrap();
        assert_eq!(recovered.as_slice(), b"synthetic");
    }

    #[test]
    fn protect_rejects_over_bound() {
        assert_eq!(
            protect_bounded(&[0u8; 8], 4),
            Err(DpapiError::SizeOutOfBounds)
        );
    }

    #[test]
    fn unprotect_rejects_over_bound_blob() {
        let protected = protect_bounded(b"synthetic", 64).unwrap();
        assert_eq!(
            unprotect_bounded(&protected, 4),
            Err(DpapiError::SizeOutOfBounds)
        );
    }
}
