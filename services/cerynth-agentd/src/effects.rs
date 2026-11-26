//! Durable at-most-once submission at the actual Guard broker boundary.
use crate::{AgentService, registry};
use anyhow::{Context, Result, ensure};
use cerynth_agent_protocols::guard::GuardClient;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::Mutex,
};
use uuid::Uuid;

pub(crate) fn root() -> PathBuf {
    PathBuf::from(format!(
        "/run/user/{}/cerynth-effects",
        nix::unistd::getuid()
    ))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: Uuid,
    session_id: Uuid,
    fingerprint: String,
    response: Option<Value>,
}
pub(crate) struct Journal {
    path: PathBuf,
    records: Mutex<BTreeMap<Uuid, Record>>,
    _lock: File,
}

fn persist(path: &Path, records: &BTreeMap<Uuid, Record>) -> Result<()> {
    let bytes = serde_json::to_vec(&records.values().collect::<Vec<_>>())?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "effect journal full; no new execution permitted"
    );
    let directory = path.parent().context("effect directory missing")?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

impl Journal {
    pub(crate) fn open(directory: &Path) -> Result<Self> {
        registry::private_directory(directory)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(directory.join("effects.lock"))?;
        lock.try_lock_exclusive()
            .context("effect journal already owned")?;
        let path = directory.join("effects.json");
        let mut records = BTreeMap::new();
        if path.exists() {
            let metadata = std::fs::symlink_metadata(&path)?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == nix::unistd::getuid().as_raw()
                    && metadata.mode() & 0o777 == 0o600
                    && metadata.len() <= 4 * 1024 * 1024,
                "invalid effect journal"
            );
            let stored: Vec<Record> = serde_json::from_slice(&std::fs::read(&path)?)?;
            ensure!(stored.len() <= 4096, "effect journal record bound exceeded");
            for record in stored {
                ensure!(
                    record.fingerprint.len() == 64
                        && record.fingerprint.bytes().all(|c| c.is_ascii_hexdigit()),
                    "invalid effect digest"
                );
                ensure!(
                    records.insert(record.id, record).is_none(),
                    "duplicate persisted operation ID"
                );
            }
        }
        Ok(Self {
            path,
            records: Mutex::new(records),
            _lock: lock,
        })
    }

    pub(crate) async fn known(&self, id: Uuid) -> bool {
        self.records.lock().await.contains_key(&id)
    }

    pub(crate) async fn decide(
        &self,
        session: Uuid,
        client: &GuardClient,
        request: Value,
    ) -> Result<Value> {
        let id: Uuid = serde_json::from_value(request["request_id"].clone())
            .context("stable tool request_id required")?;
        ensure!(
            request["session_id"] == session.to_string()
                && request["schema"] == "cerynth.guard/v1alpha1",
            "tool session/schema mismatch"
        );
        let bytes = serde_json::to_vec(&request)?;
        ensure!(bytes.len() < 60 * 1024, "tool action exceeds limit");
        let fingerprint = format!("{:x}", Sha256::digest(bytes));
        // Keep the journal claim through dispatch and durable completion. A
        // concurrent duplicate can only observe the same receipt or unknown state.
        let mut records = self.records.lock().await;
        if let Some(record) = records.get(&id) {
            ensure!(
                record.session_id == session && record.fingerprint == fingerprint,
                "tool operation ID reused with different details"
            );
            return record
                .response
                .clone()
                .context("tool outcome unknown; automatic resubmission prohibited");
        }
        ensure!(records.len() < 4096, "effect operation limit reached");
        records.insert(
            id,
            Record {
                id,
                session_id: session,
                fingerprint,
                response: None,
            },
        );
        persist(&self.path, &records)?;
        let response = client
            .request(json!({"op":"decide","request":request}))
            .await?;
        ensure!(
            response["type"] == "decision"
                && response["decision"]["request_id"] == id.to_string()
                && response["decision"]["session_id"] == session.to_string(),
            "invalid Guard action receipt; outcome unknown"
        );
        ensure!(
            serde_json::to_vec(&response)?.len() <= 64 * 1024,
            "tool receipt exceeds bound; outcome unknown"
        );
        records.get_mut(&id).context("effect claim lost")?.response = Some(response.clone());
        persist(&self.path, &records)?;
        Ok(response)
    }

    pub(crate) async fn await_approval(
        &self,
        session: Uuid,
        client: &GuardClient,
        id: Uuid,
    ) -> Result<Value> {
        let known = self.records.lock().await.values().any(|r| {
            r.session_id == session
                && r.response
                    .as_ref()
                    .is_some_and(|v| v["decision"]["approval"]["approval_id"] == id.to_string())
        });
        ensure!(
            known,
            "approval not bound to this session's immutable tool action"
        );
        // Await is read-only in the pinned Guard protocol. Never recreate a
        // missing approval by resubmitting its external action.
        let response = client
            .request(json!({"op":"await_approval","approval_id":id}))
            .await?;
        if response["type"] == "decision" {
            let request: Uuid = serde_json::from_value(response["decision"]["request_id"].clone())?;
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&request)
                .context("approval operation disappeared")?;
            ensure!(
                record.session_id == session
                    && response["decision"]["session_id"] == session.to_string(),
                "approval receipt session mismatch"
            );
            ensure!(
                serde_json::to_vec(&response)?.len() <= 64 * 1024,
                "approval receipt too large"
            );
            record.response = Some(response.clone());
            persist(&self.path, &records)?;
        }
        Ok(response)
    }

    pub(crate) async fn status(&self, session: Uuid) -> Value {
        json!({"operations":self.records.lock().await.values().filter(|r|r.session_id==session).map(|r|
            json!({"operation_id":r.id,"fingerprint":r.fingerprint,"outcome":if r.response.is_some(){"receipt_recorded"}else{"unknown"},
                "receipt":r.response,"automatic_resubmission":false})).collect::<Vec<_>>()})
    }
}

pub(crate) struct Gateway {
    task: tokio::task::JoinHandle<()>,
    path: PathBuf,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn gateway(
    session: Uuid,
    journal: Arc<Journal>,
    client: GuardClient,
) -> Result<Gateway> {
    registry::private_directory(&root())?;
    let path = root().join(format!("{session}.sock"));
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let task = tokio::spawn(async move {
        let mut clients = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted=listener.accept(),if clients.len()<8=>{let Ok((stream,_))=accepted else {break;};
                    let journal=journal.clone();let client=client.clone();clients.spawn(async move {let _=connection(stream,session,journal,client).await;});},
                Some(_)=clients.join_next(),if !clients.is_empty()=>{},
            }
        }
    });
    Ok(Gateway { task, path })
}

async fn connection(
    mut stream: UnixStream,
    session: Uuid,
    journal: Arc<Journal>,
    client: GuardClient,
) -> Result<()> {
    ensure!(
        stream.peer_cred()?.uid() == nix::unistd::getuid().as_raw(),
        "tool peer mismatch"
    );
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    for _ in 0..32 {
        let mut bytes = Vec::new();
        let count = tokio::time::timeout(
            Duration::from_secs(5),
            (&mut reader).take(65537).read_until(b'\n', &mut bytes),
        )
        .await??;
        if count == 0 {
            break;
        }
        ensure!(
            bytes.len() <= 65536 && bytes.last() == Some(&b'\n'),
            "invalid tool frame"
        );
        let result = async {
            let request: Value = serde_json::from_slice(&bytes)?;
            match request["op"].as_str() {
                Some("decide") => {
                    journal
                        .decide(session, &client, request["request"].clone())
                        .await
                }
                Some("await_approval") => {
                    journal
                        .await_approval(
                            session,
                            &client,
                            serde_json::from_value(request["approval_id"].clone())?,
                        )
                        .await
                }
                _ => Ok(json!({"type":"error","code":"caller_not_authorized",
                    "message":"only session tool decisions and bound approval reads are supported"})),
            }
        }
        .await;
        let response=result.unwrap_or_else(|_|json!({"type":"error","code":"effect_rejected_or_unknown","message":"tool request rejected or outcome unknown; inspect owner tool_effects evidence before retry"}));
        let mut output = serde_json::to_vec(&response)?;
        output.push(b'\n');
        writer.write_all(&output).await?;
    }
    Ok(())
}

impl AgentService {
    pub(crate) async fn tool_await(&self, id: Uuid, approval: Uuid) -> Result<Value> {
        let session = self.runtime.lock().await.registry.get(id)?;
        let client = GuardClient::new(
            session
                .execution
                .guard_socket
                .context("Guard is not ready")?,
        )?;
        self.effects.await_approval(id, &client, approval).await
    }
    pub(crate) async fn recover_effect_sockets(&self) -> Result<()> {
        use std::os::unix::fs::FileTypeExt;
        for session in self.runtime.lock().await.registry.list() {
            let path = root().join(format!("{}.sock", session.id));
            if let Ok(metadata) = std::fs::symlink_metadata(&path) {
                ensure!(
                    metadata.file_type().is_socket() && metadata.uid() == session.owner_uid,
                    "unexpected effect socket path"
                );
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }
    pub(crate) async fn tool_decide(&self, id: Uuid, request: Value) -> Result<Value> {
        let session = self.runtime.lock().await.registry.get(id)?;
        let operation: Uuid = serde_json::from_value(request["request_id"].clone())?;
        ensure!(
            !session.state.is_terminal() || self.effects.known(operation).await,
            "terminal session cannot start a tool action"
        );
        ensure!(
            session.spec.runtime.guard.is_some(),
            "tool action requires Guard"
        );
        let client = GuardClient::new(
            session
                .execution
                .guard_socket
                .context("Guard is not ready")?,
        )?;
        self.effects.decide(id, &client, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn external_effect_submission_is_durable_and_unknown_never_resubmits() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let journal = Arc::new(Journal::open(&state)?);
        let path = directory.path().join("guard.sock");
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                BufReader::new(&mut stream)
                    .read_until(b'\n', &mut bytes)
                    .await
                    .unwrap();
                seen.fetch_add(1, Ordering::SeqCst);
                if index == 1 {
                    continue;
                } // Side effect happened, acknowledgement lost.
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                let response = json!({"type":"decision","decision":{"request_id":request["request"]["request_id"],
                    "session_id":request["request"]["session_id"],"decision":"allow","enforcement":"enforced","result":{"executed":true}}});
                stream
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let client = GuardClient::new(path)?;
        let session = Uuid::new_v4();
        let request = json!({"schema":"cerynth.guard/v1alpha1","session_id":session,"request_id":Uuid::new_v4(),
            "action":{"kind":"git.push","target":"origin:refs/heads/main","args":{"commit":"0".repeat(40)}},"context":{"origin":"agent"}});
        let (a, b) = tokio::join!(
            journal.decide(session, &client, request.clone()),
            journal.decide(session, &client, request.clone())
        );
        assert_eq!(a?, b?);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let mut changed = request.clone();
        changed["action"]["args"]["commit"] = json!("1".repeat(40));
        assert!(journal.decide(session, &client, changed).await.is_err());
        let mut lost = request.clone();
        lost["request_id"] = json!(Uuid::new_v4());
        assert!(
            journal
                .decide(session, &client, lost.clone())
                .await
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
        drop(journal);
        let recovered = Journal::open(&state)?;
        assert!(
            recovered
                .decide(session, &client, lost)
                .await
                .unwrap_err()
                .to_string()
                .contains("unknown")
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(
            recovered.decide(session, &client, request.clone()).await?["decision"]["result"]["executed"],
            true
        );
        assert!(
            recovered
                .decide(Uuid::new_v4(), &client, request)
                .await
                .is_err()
        );
        server.await?;
        Ok(())
    }
}
