use anyhow::{Context, Result, ensure};
use cerynth_model_api::OllamaClient;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OllamaLaunch {
    pub id: String,
    pub binary: PathBuf,
    pub models_directory: PathBuf,
    pub endpoint: String,
    pub context_length: u32,
    #[serde(default = "default_parallel")]
    pub parallel_requests: u32,
}

fn default_parallel() -> u32 {
    1
}

struct OwnedBackend {
    child: Child,
    unit: Option<String>,
    endpoint: String,
}

impl Drop for OwnedBackend {
    fn drop(&mut self) {
        if let Some(unit) = self.unit.take() {
            let _ = crate::backend_scope::stop(&unit);
        }
        let _ = self.child.start_kill();
    }
}

#[derive(Default)]
pub struct BackendManager {
    processes: Mutex<BTreeMap<String, OwnedBackend>>,
    journal: Mutex<crate::backend_scope::Journal>,
}

impl BackendManager {
    pub(crate) fn persistent(directory: &std::path::Path) -> Result<Self> {
        Ok(Self {
            processes: Mutex::new(BTreeMap::new()),
            journal: Mutex::new(crate::backend_scope::Journal::persistent(directory)?),
        })
    }
    #[allow(clippy::too_many_lines)]
    pub async fn start(&self, spec: &OllamaLaunch) -> Result<()> {
        ensure!(
            !spec.id.is_empty()
                && spec.id.len() <= 128
                && (4096..=65536).contains(&spec.context_length),
            "invalid backend launch configuration"
        );
        ensure!(
            (1..=8).contains(&spec.parallel_requests),
            "backend parallelism must be 1..8"
        );
        ensure!(
            spec.binary.is_absolute()
                && spec.binary.is_file()
                && spec.models_directory.is_absolute(),
            "backend executable and model directory must be absolute"
        );
        let client = OllamaClient::new(&spec.endpoint)?;
        ensure!(
            tokio::time::timeout(Duration::from_secs(1), client.models())
                .await
                .map_or(true, |r| r.is_err()),
            "endpoint is already served; refusing to take ownership"
        );
        let mut processes = self.processes.lock().await;
        ensure!(!processes.contains_key(&spec.id), "backend already owned");
        std::fs::create_dir_all(&spec.models_directory)?;
        let unit = self.journal.lock().await.reserve()?;
        let home = self.journal.lock().await.private_home(&unit)?;
        let uid = nix::unistd::getuid();
        let child = Command::new("/usr/bin/systemd-run")
            .args(crate::backend_scope::payload(&unit, &spec.binary))
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", home)
            .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{uid}/bus"),
            )
            .env("OLLAMA_HOST", &spec.endpoint)
            .env("OLLAMA_MODELS", &spec.models_directory)
            .env("OLLAMA_NO_CLOUD", "1")
            .env("OLLAMA_NUM_PARALLEL", spec.parallel_requests.to_string())
            .env("OLLAMA_MAX_LOADED_MODELS", "1")
            .env("OLLAMA_CONTEXT_LENGTH", spec.context_length.to_string())
            .env("OLLAMA_DEBUG_LOG_REQUESTS", "false")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .context("cannot start owned Ollama backend")?;
        let mut owned = OwnedBackend {
            child,
            unit: Some(unit),
            endpoint: spec.endpoint.clone(),
        };
        // Recent Ollama releases discover several driver backends before tags
        // becomes ready. Keep this bounded without rejecting a healthy cold GPU.
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                ensure!(
                    owned.child.try_wait()?.is_none(),
                    "backend exited before readiness"
                );
                if tokio::time::timeout(Duration::from_millis(300), client.models())
                    .await
                    .is_ok_and(|r| r.is_ok())
                {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("backend readiness deadline exceeded")??;
        processes.insert(spec.id.clone(), owned);
        Ok(())
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        let mut owned = self
            .processes
            .lock()
            .await
            .remove(id)
            .context("backend is not owned by this daemon")?;
        let unit = owned
            .unit
            .as_ref()
            .context("backend scope identity missing")?;
        self.journal.lock().await.release(unit)?;
        owned.unit = None;
        tokio::time::timeout(Duration::from_secs(3), owned.child.wait()).await??;
        Ok(())
    }

    pub async fn status(&self) -> BTreeMap<String, String> {
        let mut processes = self.processes.lock().await;
        let mut status: BTreeMap<String, String> = processes
            .iter_mut()
            .map(|(id, process)| {
                (
                    id.clone(),
                    match process.child.try_wait() {
                        Ok(None) => "running".into(),
                        Ok(Some(status)) => format!("exited:{status}"),
                        Err(_) => "unknown".into(),
                    },
                )
            })
            .collect();
        for unit in self
            .journal
            .lock()
            .await
            .units()
            .iter()
            .filter(|unit| !processes.values().any(|p| p.unit.as_ref() == Some(*unit)))
        {
            status.insert(
                format!("cleanup:{unit}"),
                "cleanup_pending_restart_reconciliation".into(),
            );
        }
        status
    }
    pub(crate) async fn owns(&self, id: &str, url: &str) -> bool {
        self.processes
            .lock()
            .await
            .get_mut(id)
            .is_some_and(|owned| {
                owned.child.try_wait().is_ok_and(|s| s.is_none())
                    && owned.endpoint.trim_end_matches('/') == url.trim_end_matches('/')
            })
    }
}
