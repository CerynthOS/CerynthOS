//! Append-only, hash-chained audit log (JSON Lines).
//!
//! Each record stores `prev` (the previous record's hash) and `hash` =
//! SHA-256(prev || canonical JSON of the record without `hash`). Editing,
//! deleting an interior record or reordering records breaks later hashes, which
//! [`verify`] detects. The log lives outside every sandbox, written only by
//! guardd. A valid suffix can be removed without detection unless the last hash is
//! anchored outside the log.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GENESIS: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub seq: u64,
    pub ts: u64,
    pub event: String,
    pub data: serde_json::Value,
    pub prev: String,
    pub hash: String,
}

fn record_hash(seq: u64, ts: u64, event: &str, data: &serde_json::Value, prev: &str) -> String {
    let body =
        serde_json::json!({ "seq": seq, "ts": ts, "event": event, "data": data, "prev": prev });
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update(crate::decision::canonical_json(&body));
    format!("sha256:{}", hex::encode(h.finalize()))
}

pub struct AuditLog {
    path: PathBuf,
    file: File,
    next_seq: u64,
    last_hash: String,
}

impl AuditLog {
    /// Open (or create) the log and resume the chain. Refuses to append to a
    /// log whose chain doesn't verify, so tampering can't be papered over.
    ///
    /// # Errors
    /// I/O errors, or a broken chain.
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        // SAFETY: file owns a valid fd for the duration of the call; flock
        // doesn't retain a pointer. Hold the exclusive lock until drop.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let (next_seq, last_hash) = match verify(&path) {
            Ok(v) => (v.records, v.last_hash),
            Err(VerifyError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                (0, GENESIS.to_string())
            }
            Err(e) => anyhow::bail!("audit log {} does not verify: {e}", path.display()),
        };
        Ok(Self {
            path,
            file,
            next_seq,
            last_hash,
        })
    }

    /// Append one record and flush it to disk.
    ///
    /// # Errors
    /// I/O errors.
    pub fn append(&mut self, event: &str, data: serde_json::Value) -> anyhow::Result<AuditRecord> {
        let ts = crate::decision::now_secs();
        let hash = record_hash(self.next_seq, ts, event, &data, &self.last_hash);
        let rec = AuditRecord {
            seq: self.next_seq,
            ts,
            event: event.into(),
            data,
            prev: self.last_hash.clone(),
            hash,
        };
        let mut line = serde_json::to_vec(&rec)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        self.next_seq += 1;
        self.last_hash.clone_from(&rec.hash);
        Ok(rec)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub records: u64,
    pub last_hash: String,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("line {line}: not a valid record: {msg}")]
    Malformed { line: u64, msg: String },
    #[error("record {seq}: chain broken (expected prev {expected}, found {found})")]
    BrokenChain {
        seq: u64,
        expected: String,
        found: String,
    },
    #[error("record {seq}: hash mismatch (record was modified)")]
    HashMismatch { seq: u64 },
    #[error("record {found}: out of sequence (expected {expected})")]
    Sequence { expected: u64, found: u64 },
}

/// Verify a whole log file.
///
/// # Errors
/// The first problem found.
pub fn verify(path: impl AsRef<Path>) -> Result<Verified, VerifyError> {
    let reader = BufReader::new(File::open(path)?);
    let mut prev = GENESIS.to_string();
    let mut n = 0u64;
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: AuditRecord = serde_json::from_str(&line).map_err(|e| VerifyError::Malformed {
            line: i as u64 + 1,
            msg: e.to_string(),
        })?;
        if rec.seq != n {
            return Err(VerifyError::Sequence {
                expected: n,
                found: rec.seq,
            });
        }
        if rec.prev != prev {
            return Err(VerifyError::BrokenChain {
                seq: rec.seq,
                expected: prev,
                found: rec.prev,
            });
        }
        if record_hash(rec.seq, rec.ts, &rec.event, &rec.data, &rec.prev) != rec.hash {
            return Err(VerifyError::HashMismatch { seq: rec.seq });
        }
        prev = rec.hash;
        n += 1;
    }
    Ok(Verified {
        records: n,
        last_hash: prev,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "cerynth-audit-{name}-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn chain_verifies_and_resumes() {
        let p = tmp("ok");
        {
            let mut log = AuditLog::open(&p).unwrap();
            log.append("guard.decision", serde_json::json!({"decision":"deny"}))
                .unwrap();
            log.append("guard.decision", serde_json::json!({"decision":"allow"}))
                .unwrap();
        }
        let mut log = AuditLog::open(&p).unwrap(); // reopen: chain continues
        log.append("guard.approval.resolved", serde_json::json!({}))
            .unwrap();
        assert_eq!(verify(&p).unwrap().records, 3);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn only_one_writer_can_own_a_log() {
        let p = tmp("lock");
        let log = AuditLog::open(&p).unwrap();
        assert!(AuditLog::open(&p).is_err());
        drop(log);
        assert!(AuditLog::open(&p).is_ok());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn tampering_is_detected() {
        let p = tmp("tamper");
        let mut log = AuditLog::open(&p).unwrap();
        for i in 0..3 {
            log.append(
                "guard.decision",
                serde_json::json!({"i": i, "decision": "deny"}),
            )
            .unwrap();
        }
        drop(log);
        let text = std::fs::read_to_string(&p).unwrap();

        // Edit a field.
        std::fs::write(
            &p,
            text.replacen(r#""decision":"deny""#, r#""decision":"allow""#, 1),
        )
        .unwrap();
        assert!(matches!(
            verify(&p),
            Err(VerifyError::HashMismatch { seq: 0 })
        ));
        // A tampered log refuses to be appended to.
        assert!(AuditLog::open(&p).is_err());

        // Delete a record.
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&p, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(verify(&p).is_err());

        // Reorder.
        std::fs::write(&p, format!("{}\n{}\n{}\n", lines[1], lines[0], lines[2])).unwrap();
        assert!(verify(&p).is_err());
        let _ = std::fs::remove_file(p);
    }
}
