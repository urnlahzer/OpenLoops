//! The protected-blob-store file layer: root resolution and atomic replace.
//!
//! `protected_blob_store` gives an exact Win32 recipe (known-folder
//! resolution, handle-based reparse/hard-link/ADS rejection,
//! `MoveFileExW`/`ReplaceFileW`, an owner-only DACL) that needs FFI surface
//! well beyond the three calls [`crate::dpapi_ffi`] is scoped to under this
//! story's sanctioned unsafe boundary (see that module's doc comment). This
//! module implements the same *shape* — same-directory `CREATE_NEW` temp
//! file, flush, atomic rename over the canonical target, reopen-and-validate
//! — using only safe `std::fs` APIs, and is explicit below about exactly
//! where that is a narrower guarantee than the contract's Win32 recipe.
//!
//! # Honest gaps versus `protected_blob_store`
//!
//! * **Root resolution.** `protected_blob_store.root` requires
//!   `FOLDERID_LocalAppData` resolved via the Win32 known-folder API ("no
//!   environment expansion ... fallback"). This module resolves the same
//!   directory via `%LOCALAPPDATA%` (`std::env::var`), which is how Windows
//!   itself populates that variable for the calling user but is textually an
//!   environment-variable read, which the contract's wording rules out.
//!   Closing this gap needs `SHGetKnownFolderPath`
//!   (`Win32_UI_Shell`), which is outside this story's sanctioned unsafe
//!   surface.
//! * **Atomic replace.** `write_steps` specifies `MoveFileExW` (no
//!   `REPLACE_EXISTING`, `MOVEFILE_WRITE_THROUGH`) when no canonical target
//!   exists and `ReplaceFileW` (`lpBackupFileName` NULL,
//!   `dwReplaceFlags` zero) when one does. `std::fs::rename` on Windows
//!   calls `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH`
//!   unconditionally (see the Rust standard library's Windows `rename`
//!   implementation), which does replace an existing target atomically but
//!   does not distinguish the two cases the contract wants distinguished,
//!   and is not `ReplaceFileW` (so it does not preserve the target's
//!   security attributes/backup semantics the way `ReplaceFileW` does).
//! * **DACL / reparse-point / hard-link / ADS checks.** `directory_security`
//!   and `path_safety` require an owner-only DACL and handle-based rejection
//!   of reparse points, extra hard links, and alternate data streams. Only
//!   [`verify_owner_only_dacl`] exists here, and it always returns
//!   [`ProtectedFileError::NotYetEnforced`] rather than silently claiming to
//!   check something it does not. Closing this gap needs `Win32_Security`/
//!   `Win32_Storage_FileSystem` FFI beyond this story's sanctioned module.
//! * **Interprocess writer lock.** `write_steps` assumes "the single-account
//!   writer lock"; this module approximates it with a `CREATE_NEW` lock file
//!   in the account directory (see [`WriterLock`]), which excludes other
//!   processes using this same code path but is not a named OS mutex.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::ids::AccountBindingAad;

/// A rejected protected-file operation. No variant carries a path or an
/// `std::io::Error` message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedFileError {
    /// `%LOCALAPPDATA%` was unset or empty.
    RootUnavailable,
    /// A directory or file could not be created.
    CreateFailed,
    /// The temporary file could not be written or flushed completely.
    WriteFailed,
    /// The atomic rename over the canonical target failed.
    ReplaceFailed,
    /// The canonical target could not be reopened after replace.
    ReopenFailed,
    /// The recovered bytes exceeded the caller's bound.
    OversizedFile,
    /// A startup enumeration found an entry outside the canonical/temporary
    /// name allowlist.
    UnexpectedEntry,
    /// DACL enforcement is not yet implemented (see the module doc comment);
    /// this is a typed, honest placeholder, never a silent pass.
    NotYetEnforced,
}

/// Resolves `%LOCALAPPDATA%/OpenLoops/protected-v1` — see the module doc
/// comment's "Root resolution" gap.
///
/// # Errors
///
/// Returns [`ProtectedFileError::RootUnavailable`].
pub fn root_directory() -> Result<PathBuf, ProtectedFileError> {
    let local_app_data =
        std::env::var_os("LOCALAPPDATA").ok_or(ProtectedFileError::RootUnavailable)?;
    if local_app_data.is_empty() {
        return Err(ProtectedFileError::RootUnavailable);
    }
    Ok(PathBuf::from(local_app_data)
        .join("OpenLoops")
        .join("protected-v1"))
}

/// `protected_blob_store.account_directory`: lowercase hex of the 16-byte
/// `account_binding_aad`, exactly 32 ASCII characters.
#[must_use]
pub fn account_directory(root: &Path, account_binding_aad: AccountBindingAad) -> PathBuf {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(32);
    for byte in account_binding_aad.as_bytes() {
        let _ = write!(hex, "{byte:02x}");
    }
    root.join(hex)
}

/// Ensures `dir` and every ancestor up to (and including) it exists.
///
/// # Errors
///
/// Returns [`ProtectedFileError::CreateFailed`].
pub fn ensure_directory(dir: &Path) -> Result<(), ProtectedFileError> {
    std::fs::create_dir_all(dir).map_err(|_| ProtectedFileError::CreateFailed)
}

/// Writes `bytes` to `target` via a same-directory `create_new` temporary
/// file, flush, and atomic rename — see the module doc comment for exactly
/// how this differs from `write_steps`' `MoveFileExW`/`ReplaceFileW` recipe.
///
/// # Errors
///
/// Returns [`ProtectedFileError::CreateFailed`],
/// [`ProtectedFileError::WriteFailed`], or
/// [`ProtectedFileError::ReplaceFailed`].
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), ProtectedFileError> {
    let dir = target.parent().ok_or(ProtectedFileError::CreateFailed)?;
    ensure_directory(dir)?;
    let mut suffix = [0u8; 16];
    getrandom::fill(&mut suffix).map_err(|_| ProtectedFileError::CreateFailed)?;
    let mut hex_suffix = String::with_capacity(32);
    for byte in suffix {
        use std::fmt::Write as _;
        let _ = write!(hex_suffix, "{byte:02x}");
    }
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ProtectedFileError::CreateFailed)?;
    let temp_path = dir.join(format!("{file_name}.new.{hex_suffix}"));

    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|_| ProtectedFileError::CreateFailed)?;
        file.write_all(bytes)
            .map_err(|_| ProtectedFileError::WriteFailed)?;
        file.sync_all()
            .map_err(|_| ProtectedFileError::WriteFailed)?;
    }

    if std::fs::rename(&temp_path, target).is_ok() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(&temp_path);
        Err(ProtectedFileError::ReplaceFailed)
    }
}

/// Reads `target` in full, rejecting anything over `max_len` bytes.
///
/// Returns `Ok(None)` when `target` does not exist at all — the ordinary,
/// valid "no protected state yet" case, per `startup_recovery`'s "missing
/// ... canonical state enters non-mutating recovery" (the recovery decision
/// itself is [`crate::state_root`]'s, not this layer's).
///
/// # Errors
///
/// Returns [`ProtectedFileError::OversizedFile`] or
/// [`ProtectedFileError::ReopenFailed`] for any other read failure.
pub fn read_bounded(target: &Path, max_len: u64) -> Result<Option<Vec<u8>>, ProtectedFileError> {
    let metadata = match std::fs::metadata(target) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ProtectedFileError::ReopenFailed),
    };
    if metadata.len() > max_len {
        return Err(ProtectedFileError::OversizedFile);
    }
    std::fs::read(target)
        .map(Some)
        .map_err(|_| ProtectedFileError::ReopenFailed)
}

/// Always returns [`ProtectedFileError::NotYetEnforced`] — see the module
/// doc comment. Calling this and checking its result documents exactly
/// where the contract's owner-only DACL requirement is not yet met, rather
/// than omitting the check entirely.
///
/// # Errors
///
/// Always returns [`ProtectedFileError::NotYetEnforced`].
pub fn verify_owner_only_dacl(_path: &Path) -> Result<(), ProtectedFileError> {
    Err(ProtectedFileError::NotYetEnforced)
}

/// An approximate single-account writer-lock guard: a `CREATE_NEW` lock file
/// held for the guard's lifetime and removed on drop. See the module doc
/// comment's "Interprocess writer lock" gap for what this does not provide.
pub struct WriterLock {
    path: PathBuf,
}

impl WriterLock {
    /// Acquires the writer lock inside `account_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtectedFileError::CreateFailed`] if the lock is already
    /// held or the directory is unavailable.
    pub fn acquire(account_dir: &Path) -> Result<Self, ProtectedFileError> {
        ensure_directory(account_dir)?;
        let path = account_dir.join("writer.lock");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| ProtectedFileError::CreateFailed)?;
        Ok(Self { path })
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::{ProtectedFileError, WriterLock, account_directory, read_bounded, write_atomic};
    use crate::ids::RandomId;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openloops-protected-file-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn account_directory_is_exactly_32_lowercase_hex_characters() {
        let root = std::path::Path::new("root");
        let account = RandomId::from_random_bytes([0xABu8; 16]).unwrap();
        let dir = account_directory(root, account);
        let name = dir.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 32);
        assert!(
            name.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = temp_dir("roundtrip");
        let target = dir.join("state-root.dpapi");
        write_atomic(&target, b"synthetic bytes").unwrap();
        let read_back = read_bounded(&target, 65536).unwrap();
        assert_eq!(read_back, Some(b"synthetic bytes".to_vec()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_replaces_an_existing_target() {
        let dir = temp_dir("replace");
        let target = dir.join("state-root.dpapi");
        write_atomic(&target, b"first").unwrap();
        write_atomic(&target, b"second").unwrap();
        assert_eq!(
            read_bounded(&target, 65536).unwrap(),
            Some(b"second".to_vec())
        );
        // No leftover `.new.` temporary files after a successful replace.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".new."))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_bounded_returns_none_for_missing_file() {
        let dir = temp_dir("missing");
        let target = dir.join("state-root.dpapi");
        assert_eq!(read_bounded(&target, 65536).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_bounded_rejects_oversized_file() {
        let dir = temp_dir("oversized");
        let target = dir.join("state-root.dpapi");
        write_atomic(&target, &[0u8; 32]).unwrap();
        assert_eq!(
            read_bounded(&target, 4),
            Err(ProtectedFileError::OversizedFile)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writer_lock_excludes_a_second_acquisition() {
        let dir = temp_dir("lock");
        let first = WriterLock::acquire(&dir).unwrap();
        assert!(matches!(
            WriterLock::acquire(&dir),
            Err(ProtectedFileError::CreateFailed)
        ));
        drop(first);
        assert!(WriterLock::acquire(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dacl_verification_is_an_honest_not_yet_enforced_gap() {
        let dir = temp_dir("dacl");
        assert_eq!(
            super::verify_owner_only_dacl(&dir),
            Err(ProtectedFileError::NotYetEnforced)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
