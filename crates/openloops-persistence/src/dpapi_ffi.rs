//! The one sanctioned unsafe module in this crate.
//!
//! `dependency_policy.unsafe_boundary`: "future minimal Windows FFI must be
//! isolated documented tested and independently audited; current workspace
//! remains unsafe-code-free". This module is that isolation boundary: it
//! wraps exactly three Win32 calls — `CryptProtectData`, `CryptUnprotectData`,
//! and `LocalFree` — behind safe Rust signatures, and nothing else in this
//! crate (or the workspace) is allowed to contain `unsafe`. See
//! `crates/openloops-persistence/Cargo.toml` for how the crate-local lint
//! table narrows the workspace's `unsafe_code = "forbid"` to `deny` so that
//! exactly the one module-level `#![allow(unsafe_code)]` below is possible;
//! `forbid` itself cannot be locally lowered by design.
//!
//! # Audit boundary
//!
//! * **Scope.** Only `os_protection.primitive` (`CryptProtectData`and
//!   `CryptUnprotectData`) plus the `LocalFree` calls their own documentation
//!   requires. No known-folder resolution, ACL read, or file-replace FFI
//!   lives here (see `crate::protected_file` for the honest gap that leaves).
//! * **Flags.** `os_protection.scope`/`flags`: current user only, never
//!   `CRYPTPROTECT_LOCAL_MACHINE`; `CRYPTPROTECT_UI_FORBIDDEN` is always set
//!   so the call can never block on a prompt. `description` and
//!   `optional_entropy` are always `NULL` (`os_protection.description`/
//!   `optional_entropy`).
//! * **Memory ownership.** Both `CryptProtectData` and `CryptUnprotectData`
//!   allocate `pDataOut->pbData` via `LocalAlloc` internally and require the
//!   caller to free it with `LocalFree`
//!   ([`SRC-MS-DPAPI-PROTECT`]/[`SRC-MS-DPAPI-UNPROTECT`]). Both wrappers
//!   below copy that buffer into an owned `Vec<u8>` and call `LocalFree`
//!   exactly once on every path — success and every early return — using a
//!   drop guard ([`LocalAllocGuard`]) so a future edit that adds a new early
//!   return cannot reintroduce a leak or a double-free by omission.
//! * **Zeroization.** The copied buffer is wrapped in `zeroize::Zeroizing`
//!   before any further processing so the plaintext key-bundle bytes are
//!   cleared on every exit path, including error paths.
//!
//! [`SRC-MS-DPAPI-PROTECT`]: https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata
//! [`SRC-MS-DPAPI-UNPROTECT`]: https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptunprotectdata

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

/// A rejected DPAPI call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DpapiFfiError {
    /// The input was empty or exceeded `u32::MAX` bytes (the Win32 blob
    /// length field is 32-bit).
    InputOutOfRange,
    /// `CryptProtectData` returned failure (`GetLastError` is not surfaced:
    /// the sanitized error taxonomy never carries OS diagnostic text).
    ProtectFailed,
    /// `CryptUnprotectData` returned failure — including "this is not a
    /// DPAPI blob", "a different user/machine protected it", and "the OS
    /// rejected it for any other reason". This crate cannot and does not
    /// distinguish those cases from the return value alone.
    UnprotectFailed,
}

/// Frees exactly one `LocalAlloc`-backed pointer, exactly once, on drop.
///
/// Wrapping the raw pointer in this guard immediately after the FFI call
/// returns means every subsequent early return (`?`, bounds check, `Vec`
/// allocation failure) still runs the drop and frees the buffer; there is no
/// code path between a successful call and the guard's construction where an
/// early return could skip the free or double-free it.
struct LocalAllocGuard(*mut u8);

impl Drop for LocalAllocGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was returned by a successful `CryptProtectData`
            // or `CryptUnprotectData` call as `pDataOut.pbData`, is owned
            // exclusively by this guard (never aliased, never freed
            // elsewhere), and `LocalFree` accepts any `HLOCAL` including one
            // obtained this way; `Drop::drop` runs at most once per value.
            unsafe {
                LocalFree(self.0.cast());
            }
        }
    }
}

/// Calls `CryptProtectData` on `plaintext` with no description, no prompt,
/// no optional entropy, current-user scope, and `CRYPTPROTECT_UI_FORBIDDEN`.
///
/// # Errors
///
/// Returns [`DpapiFfiError::InputOutOfRange`] for an empty or oversized
/// input, or [`DpapiFfiError::ProtectFailed`] if the OS call fails.
pub fn protect(plaintext: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, DpapiFfiError> {
    if plaintext.is_empty() || plaintext.len() > u32::MAX as usize {
        return Err(DpapiFfiError::InputOutOfRange);
    }
    let mut input_blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plaintext.len()).map_err(|_| DpapiFfiError::InputOutOfRange)?,
        pbData: plaintext.as_ptr().cast_mut(),
    };
    let mut output_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: core::ptr::null_mut(),
    };

    // SAFETY: `input_blob.pbData`/`cbData` describe the live, in-bounds
    // `plaintext` slice for the duration of this call only. `szDataDescr`,
    // `pOptionalEntropy`, and `pvReserved` are `NULL`/absent per
    // `os_protection.description`/`optional_entropy`; `pPromptStruct` is
    // `NULL` and `CRYPTPROTECT_UI_FORBIDDEN` is set so the call can never
    // display UI. `output_blob` is a valid, exclusively-owned out param.
    let ok = unsafe {
        CryptProtectData(
            &raw mut input_blob,
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output_blob,
        )
    };
    if ok == 0 {
        return Err(DpapiFfiError::ProtectFailed);
    }
    let guard = LocalAllocGuard(output_blob.pbData);
    let len = output_blob.cbData as usize;
    let mut owned = zeroize::Zeroizing::new(Vec::with_capacity(len));
    if len > 0 {
        // SAFETY: on success, `output_blob.pbData` points to `cbData`
        // initialized bytes owned by this call (per `CryptProtectData`
        // documentation), and `guard` keeps that allocation alive for the
        // duration of this read; the bytes are copied out before the guard
        // frees the source.
        owned.extend_from_slice(unsafe { core::slice::from_raw_parts(output_blob.pbData, len) });
    }
    drop(guard);
    Ok(owned)
}

/// Calls `CryptUnprotectData` on `blob`, requiring a `NULL` description and
/// no optional entropy, and returning the recovered plaintext.
///
/// # Errors
///
/// Returns [`DpapiFfiError::InputOutOfRange`] for an empty or oversized
/// input, or [`DpapiFfiError::UnprotectFailed`] if the OS call fails for any
/// reason (wrong user, wrong machine, corrupt blob, or any other cause; the
/// OS does not distinguish these to the caller and this crate does not
/// synthesize a distinction it cannot verify).
pub fn unprotect(blob: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, DpapiFfiError> {
    if blob.is_empty() || blob.len() > u32::MAX as usize {
        return Err(DpapiFfiError::InputOutOfRange);
    }
    let mut input_blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(blob.len()).map_err(|_| DpapiFfiError::InputOutOfRange)?,
        pbData: blob.as_ptr().cast_mut(),
    };
    let mut output_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: core::ptr::null_mut(),
    };
    // SAFETY: same argument shape as `protect`'s call, in reverse. The
    // description out-parameter is passed as NULL so the OS never allocates
    // a description string for this call; `protect` always stores a NULL
    // description, and requesting it back would obligate this function to
    // `LocalFree` an allocation it has no use for.
    let ok = unsafe {
        CryptUnprotectData(
            &raw mut input_blob,
            core::ptr::null_mut(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output_blob,
        )
    };
    if ok == 0 {
        return Err(DpapiFfiError::UnprotectFailed);
    }
    let guard = LocalAllocGuard(output_blob.pbData);
    let len = output_blob.cbData as usize;
    let mut owned = zeroize::Zeroizing::new(Vec::with_capacity(len));
    if len > 0 {
        // SAFETY: same argument as in `protect`: `output_blob.pbData` is a
        // valid `cbData`-byte allocation owned by this call, kept alive by
        // `guard` for the duration of this read.
        owned.extend_from_slice(unsafe { core::slice::from_raw_parts(output_blob.pbData, len) });
    }
    drop(guard);
    Ok(owned)
}

#[cfg(test)]
mod tests {
    use super::{DpapiFfiError, protect, unprotect};

    /// Real current-user DPAPI round trip on this machine, with synthetic
    /// key material only (never a real secret).
    #[test]
    fn protect_then_unprotect_round_trips_synthetic_bytes() {
        let plaintext = b"synthetic-dpapi-round-trip-vector";
        let protected = protect(plaintext).expect("DPAPI protect available on this machine");
        assert_ne!(protected.as_slice(), plaintext.as_slice());
        let recovered = unprotect(&protected).expect("DPAPI unprotect available on this machine");
        assert_eq!(recovered.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn protect_rejects_empty_input() {
        assert_eq!(protect(&[]), Err(DpapiFfiError::InputOutOfRange));
    }

    #[test]
    fn unprotect_rejects_empty_input() {
        assert_eq!(unprotect(&[]), Err(DpapiFfiError::InputOutOfRange));
    }

    /// `blob_validation`: a structurally-tampered protected blob must fail
    /// closed rather than unprotect to garbage plaintext.
    #[test]
    fn unprotect_rejects_tampered_blob() {
        let plaintext = b"synthetic-tamper-vector";
        let mut protected = protect(plaintext)
            .expect("DPAPI protect available on this machine")
            .to_vec();
        let last = protected.len() - 1;
        protected[last] ^= 0xFF;
        assert_eq!(unprotect(&protected), Err(DpapiFfiError::UnprotectFailed));
    }

    /// This crate cannot simulate a genuinely different Windows user or
    /// machine identity in-process (that requires a second real principal),
    /// so cross-user rejection is exercised only via the tampered-blob path
    /// above, which the OS handles identically at the API surface: any blob
    /// this process's DPAPI master key cannot authenticate — wrong user,
    /// wrong machine, or corrupted — returns the same `UnprotectFailed`.
    /// True cross-user/cross-machine coverage is an honest gap for a
    /// multi-principal integration environment, not a unit test.
    #[test]
    fn unprotect_rejects_structurally_invalid_blob() {
        assert_eq!(unprotect(&[0u8; 4]), Err(DpapiFfiError::UnprotectFailed));
    }
}
