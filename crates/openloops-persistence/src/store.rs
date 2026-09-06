//! The `SQLite` envelope store: verified pragmas, closed per-record-type
//! tables, and the transactional write primitive.
//!
//! `database_contract`: WAL journal mode, `synchronous=FULL`, foreign keys
//! on, `trusted_schema` off, `MEMORY` temp store, no extension loading, no
//! trace callbacks, one database per account binding, and "only the
//! minimum-clear envelope plus authenticated ciphertext and approved
//! nonsecret store metadata reach SQL". Every table created here has exactly
//! the [`crate::envelope::Envelope`] clear-field shape and nothing else —
//! there is no column, index, or trigger anywhere in this module that could
//! carry a decrypted logical value.
//!
//! This module never calls `Connection::load_extension_enable` (so extension
//! loading stays off — `SQLite`'s own default) and never calls `.trace()` or
//! `.profile()` (so no trace callback is ever installed).

use rusqlite::{Connection, OptionalExtension, params};

use crate::aad::EnvelopeAad;
use crate::envelope::Envelope;
use crate::ids::{AccountBindingAad, CIPHERTEXT_VERSION, RandomId, RecordType, SchemaVersion};

/// A rejected store operation. No variant carries a path, SQL text, or key
/// material; `rusqlite::Error`'s message text is deliberately discarded at
/// every call site below.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    /// The connection could not be opened.
    OpenFailed,
    /// Schema creation failed.
    SchemaFailed,
    /// A required pragma did not read back the expected value.
    PragmaMismatch(PragmaKind),
    /// A write failed (including a rejected duplicate `record_id`).
    WriteFailed,
    /// A read failed.
    ReadFailed,
    /// A stored row did not decode into a well-formed [`Envelope`].
    MalformedRow,
}

/// The specific pragma a [`StoreError::PragmaMismatch`] refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PragmaKind {
    JournalMode,
    Synchronous,
    ForeignKeys,
    TrustedSchema,
    TempStore,
}

/// One open account database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if absent) the account database at `path`, applies
    /// `database_contract`'s pragmas, verifies every one by reading it back,
    /// and creates the closed per-record-type schema if it does not already
    /// exist.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OpenFailed`], [`StoreError::PragmaMismatch`], or
    /// [`StoreError::SchemaFailed`].
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(|_| StoreError::OpenFailed)?;
        Self::apply_and_verify_pragmas(&conn)?;
        Self::create_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Opens a private in-memory database with the same pragmas and schema,
    /// for tests that do not need a real file.
    ///
    /// # Errors
    ///
    /// See [`Store::open`].
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(|_| StoreError::OpenFailed)?;
        // `synchronous=FULL` and `journal_mode=WAL` are meaningful only for a
        // real file-backed database; an in-memory database cannot use WAL
        // (SQLite silently keeps it in `memory` mode), so pragma
        // verification is skipped here and only exercised against a real
        // file in `Store::open`'s tests.
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| StoreError::SchemaFailed)?;
        Self::create_schema(&conn)?;
        Ok(Self { conn })
    }

    fn apply_and_verify_pragmas(conn: &Connection) -> Result<(), StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|_| StoreError::SchemaFailed)?;
        let journal_mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|_| StoreError::SchemaFailed)?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::PragmaMismatch(PragmaKind::JournalMode));
        }

        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|_| StoreError::SchemaFailed)?;
        let synchronous: i64 = conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .map_err(|_| StoreError::SchemaFailed)?;
        if synchronous != 2 {
            return Err(StoreError::PragmaMismatch(PragmaKind::Synchronous));
        }

        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| StoreError::SchemaFailed)?;
        let foreign_keys: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .map_err(|_| StoreError::SchemaFailed)?;
        if foreign_keys != 1 {
            return Err(StoreError::PragmaMismatch(PragmaKind::ForeignKeys));
        }

        conn.pragma_update(None, "trusted_schema", "OFF")
            .map_err(|_| StoreError::SchemaFailed)?;
        let trusted_schema: i64 = conn
            .pragma_query_value(None, "trusted_schema", |row| row.get(0))
            .map_err(|_| StoreError::SchemaFailed)?;
        if trusted_schema != 0 {
            return Err(StoreError::PragmaMismatch(PragmaKind::TrustedSchema));
        }

        conn.pragma_update(None, "temp_store", "MEMORY")
            .map_err(|_| StoreError::SchemaFailed)?;
        let temp_store: i64 = conn
            .pragma_query_value(None, "temp_store", |row| row.get(0))
            .map_err(|_| StoreError::SchemaFailed)?;
        if temp_store != 2 {
            return Err(StoreError::PragmaMismatch(PragmaKind::TempStore));
        }
        Ok(())
    }

    fn create_schema(conn: &Connection) -> Result<(), StoreError> {
        for record_type in RecordType::ALL {
            let sql = format!(
                "CREATE TABLE IF NOT EXISTS \"{table}\" (\n                    record_id BLOB PRIMARY KEY NOT NULL,\n                    account_binding_aad BLOB NOT NULL,\n                    schema_version INTEGER NOT NULL,\n                    ciphertext_version INTEGER NOT NULL,\n                    key_id BLOB NOT NULL,\n                    nonce BLOB NOT NULL,\n                    tag BLOB NOT NULL,\n                    ciphertext BLOB NOT NULL\n                ) STRICT",
                table = record_type.table_name(),
            );
            conn.execute_batch(&sql)
                .map_err(|_| StoreError::SchemaFailed)?;
        }
        Ok(())
    }

    /// Writes every envelope in `envelopes` inside one `SQLite` transaction,
    /// following `transaction_contract.post_encryption_publication_order`
    /// steps 1-4 (this crate's callers own steps 5-8, the anchor replace).
    ///
    /// Every envelope commits or none do: `Connection::transaction`'s guard
    /// rolls back on drop unless `commit()` is reached, so a panic, an
    /// error, or a process crash before the explicit commit below can never
    /// leave a partial write durable. Cross-record and cross-account
    /// reference verification ("no dangling or cross-account reference is
    /// introduced") is limited here to the account-binding check on every
    /// row against `account_binding_aad`: full relationship-graph
    /// validation (e.g. that a `reminder_link` names a `loop` that exists)
    /// is a domain concern owned by ADR-004/ADR-008/ADR-009, not this crate.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::WriteFailed`] on any row's cross-account
    /// violation or SQL failure; nothing is committed.
    pub fn write_transaction(
        &mut self,
        account_binding_aad: AccountBindingAad,
        envelopes: &[Envelope],
    ) -> Result<(), StoreError> {
        if envelopes
            .iter()
            .any(|e| e.aad.account_binding_aad.as_bytes() != account_binding_aad.as_bytes())
        {
            return Err(StoreError::WriteFailed);
        }
        let tx = self
            .conn
            .transaction()
            .map_err(|_| StoreError::WriteFailed)?;
        for envelope in envelopes {
            let table = envelope.aad.record_type.table_name();
            let sql = format!(
                "INSERT INTO \"{table}\" (record_id, account_binding_aad, schema_version, ciphertext_version, key_id, nonce, tag, ciphertext) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            );
            tx.execute(
                &sql,
                params![
                    envelope.aad.record_id.as_bytes().as_slice(),
                    envelope.aad.account_binding_aad.as_bytes().as_slice(),
                    envelope.aad.schema_version.get(),
                    CIPHERTEXT_VERSION,
                    envelope.aad.key_id.as_bytes().as_slice(),
                    envelope.nonce.as_slice(),
                    envelope.tag.as_slice(),
                    envelope.ciphertext.as_slice(),
                ],
            )
            .map_err(|_| StoreError::WriteFailed)?;
        }
        tx.commit().map_err(|_| StoreError::WriteFailed)
    }

    /// Reads every currently retained envelope for `account_binding_aad`,
    /// across every closed record-type table, for anchor recomputation
    /// ([`crate::anchor::compute`]) or full-account reload.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ReadFailed`] or [`StoreError::MalformedRow`].
    pub fn read_all(
        &self,
        account_binding_aad: AccountBindingAad,
    ) -> Result<Vec<Envelope>, StoreError> {
        let mut out = Vec::new();
        for record_type in RecordType::ALL {
            let table = record_type.table_name();
            let sql = format!(
                "SELECT record_id, account_binding_aad, schema_version, ciphertext_version, key_id, nonce, tag, ciphertext FROM \"{table}\" WHERE account_binding_aad = ?1",
            );
            let mut stmt = self
                .conn
                .prepare(&sql)
                .map_err(|_| StoreError::ReadFailed)?;
            let rows = stmt
                .query_map(params![account_binding_aad.as_bytes().as_slice()], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, u32>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                    ))
                })
                .map_err(|_| StoreError::ReadFailed)?;
            for row in rows {
                let (
                    record_id,
                    account_binding,
                    schema_version,
                    ciphertext_version,
                    key_id,
                    nonce,
                    tag,
                    ciphertext,
                ) = row.map_err(|_| StoreError::ReadFailed)?;
                if ciphertext_version != CIPHERTEXT_VERSION {
                    return Err(StoreError::MalformedRow);
                }
                let envelope = Envelope {
                    aad: EnvelopeAad {
                        key_id: to_id(&key_id)?,
                        account_binding_aad: to_id(&account_binding)?,
                        record_type,
                        record_id: to_id(&record_id)?,
                        schema_version: SchemaVersion::new(
                            core::num::NonZeroU32::new(schema_version)
                                .ok_or(StoreError::MalformedRow)?,
                        ),
                    },
                    nonce: to_array(&nonce)?,
                    tag: to_array(&tag)?,
                    ciphertext,
                };
                out.push(envelope);
            }
        }
        Ok(out)
    }

    /// Returns whether `record_id` is already present in `record_type`'s
    /// table, for idempotency checks above this layer.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ReadFailed`].
    pub fn contains(
        &self,
        record_type: RecordType,
        record_id: crate::ids::RecordId,
    ) -> Result<bool, StoreError> {
        let table = record_type.table_name();
        let sql = format!("SELECT 1 FROM \"{table}\" WHERE record_id = ?1");
        self.conn
            .query_row(&sql, params![record_id.as_bytes().as_slice()], |_| Ok(()))
            .optional()
            .map(|found| found.is_some())
            .map_err(|_| StoreError::ReadFailed)
    }
}

fn to_id(bytes: &[u8]) -> Result<RandomId, StoreError> {
    let array: [u8; 16] = bytes.try_into().map_err(|_| StoreError::MalformedRow)?;
    RandomId::from_random_bytes(array).map_err(|_| StoreError::MalformedRow)
}

fn to_array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], StoreError> {
    bytes.try_into().map_err(|_| StoreError::MalformedRow)
}

#[cfg(test)]
mod tests {
    use super::{Store, StoreError};
    use crate::aad::EnvelopeAad;
    use crate::envelope::Envelope;
    use crate::ids::{RandomId, RecordType, SchemaVersion};

    fn account() -> crate::ids::AccountBindingAad {
        RandomId::from_random_bytes([1u8; 16]).unwrap()
    }

    fn envelope(
        record_type: RecordType,
        record_id_byte: u8,
        account: crate::ids::AccountBindingAad,
    ) -> Envelope {
        Envelope {
            aad: EnvelopeAad {
                key_id: RandomId::from_random_bytes([2u8; 16]).unwrap(),
                account_binding_aad: account,
                record_type,
                record_id: RandomId::from_random_bytes([record_id_byte; 16]).unwrap(),
                schema_version: SchemaVersion::new(core::num::NonZeroU32::new(1).unwrap()),
            },
            nonce: [7u8; 12],
            tag: [8u8; 16],
            ciphertext: vec![9, 9, 9],
        }
    }

    #[test]
    fn open_creates_schema_and_verifies_pragmas_on_a_real_file() {
        let dir = std::env::temp_dir().join(format!("openloops-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.db");
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).expect("real-file store opens and verifies pragmas");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_then_read_round_trips_across_every_table() {
        let account = account();
        let mut store = Store::open_in_memory().unwrap();
        let rows = vec![
            envelope(RecordType::Loop, 1, account),
            envelope(RecordType::AccountBinding, 2, account),
            envelope(RecordType::OperationLedger, 3, account),
        ];
        store.write_transaction(account, &rows).unwrap();
        let read_back = store.read_all(account).unwrap();
        assert_eq!(read_back.len(), 3);
    }

    #[test]
    fn write_transaction_rejects_cross_account_row() {
        let account = account();
        let other = RandomId::from_random_bytes([9u8; 16]).unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let rows = vec![envelope(RecordType::Loop, 1, other)];
        assert_eq!(
            store.write_transaction(account, &rows),
            Err(StoreError::WriteFailed)
        );
        assert_eq!(store.read_all(account).unwrap().len(), 0);
    }

    #[test]
    fn duplicate_record_id_in_same_table_is_rejected_and_whole_transaction_rolls_back() {
        let account = account();
        let mut store = Store::open_in_memory().unwrap();
        let first = vec![envelope(RecordType::Loop, 1, account)];
        store.write_transaction(account, &first).unwrap();

        let second = vec![
            envelope(RecordType::AccountBinding, 5, account),
            envelope(RecordType::Loop, 1, account), // duplicate primary key
        ];
        assert_eq!(
            store.write_transaction(account, &second),
            Err(StoreError::WriteFailed)
        );
        // The whole second transaction rolled back: the AccountBinding row
        // from the failed batch must not be present either.
        assert_eq!(store.read_all(account).unwrap().len(), 1);
    }

    /// Crash-replay idempotency: a transaction that begins, inserts rows,
    /// and is dropped without an explicit commit must leave the reopened
    /// database exactly as it was before the transaction started.
    #[test]
    fn uncommitted_transaction_never_survives_reopen() {
        let dir =
            std::env::temp_dir().join(format!("openloops-store-crash-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.db");
        let _ = std::fs::remove_file(&path);
        let account = account();
        {
            let mut store = Store::open(&path).unwrap();
            store
                .write_transaction(account, &[envelope(RecordType::Loop, 1, account)])
                .unwrap();
        }
        {
            let mut store = Store::open(&path).unwrap();
            let tx = store.conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO \"account_binding\" (record_id, account_binding_aad, schema_version, ciphertext_version, key_id, nonce, tag, ciphertext) VALUES (?1, ?2, 1, 1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    [2u8; 16].as_slice(),
                    account.as_bytes().as_slice(),
                    [0u8; 12].as_slice(),
                    [0u8; 16].as_slice(),
                    [0u8; 3].as_slice(),
                ],
            )
            .unwrap();
            drop(tx); // "crash": dropped without commit -> implicit rollback
        }
        let reopened = Store::open(&path).unwrap();
        let rows = reopened.read_all(account).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "only the committed first write survives reopen"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contains_reflects_committed_rows_only() {
        let account = account();
        let mut store = Store::open_in_memory().unwrap();
        let record_id = RandomId::from_random_bytes([1u8; 16]).unwrap();
        assert!(!store.contains(RecordType::Loop, record_id).unwrap());
        store
            .write_transaction(account, &[envelope(RecordType::Loop, 1, account)])
            .unwrap();
        assert!(store.contains(RecordType::Loop, record_id).unwrap());
    }
}
