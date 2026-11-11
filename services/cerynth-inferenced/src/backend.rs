use anyhow::{Context, Result, ensure};
use cerynth_model_api::OllamaClient;
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
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
    group: i32,
}

impl Drop for OwnedBackend {
    fn drop(&mut self) {
        if self.child.id().is_some() && self.group > 1 {
            let _ = killpg(Pid::from_raw(self.group), Signal::SIGKILL);
            let _ = self.child.start_kill();
        }
    }
}

#[derive(Default)]
pub struct BackendManager {
    processes: Mutex<BTreeMap<String, OwnedBackend>>,
}

impl BackendManager {
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
        let home = spec.models_directory.join(".backend-home");
        std::fs::create_dir_all(&home)?;
        let child = Command::new(&spec.binary)
            .arg("serve")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", home)
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
        let group = i32::try_from(child.id().context("backend PID unavailable")?)?;
        let mut owned = OwnedBackend { child, group };
        tokio::time::timeout(Duration::from_secs(10), async {
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
        let _ = killpg(Pid::from_raw(owned.group), Signal::SIGTERM);
        if tokio::time::timeout(Duration::from_secs(2), owned.child.wait())
            .await
            .is_err()
        {
            let _ = killpg(Pid::from_raw(owned.group), Signal::SIGKILL);
            owned.child.kill().await?;
            owned.child.wait().await?;
        }
        Ok(())
    }

    pub async fn status(&self) -> BTreeMap<String, String> {
        let mut processes = self.processes.lock().await;
        processes
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
            .collect()
    }
}
