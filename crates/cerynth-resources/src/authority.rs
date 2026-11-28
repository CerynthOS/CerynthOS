//! Serialized durable reservations for an explicitly configured managed domain.
//! Ordinary applications and clients bypassing this owner plane are outside it.
use crate::{
    admission::{Capacity, Decision},
    global::{self, AcceleratorCapacity, Demand, OwnerSnapshot},
    pressure::Sample,
};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub capacity: Capacity,
    pub inference_limit: u32,
    pub accelerators: BTreeMap<String, AcceleratorPolicy>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceleratorPolicy {
    pub total_bytes: u64,
    pub desktop_reserve_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub owner: String,
    pub demand: Demand,
    pub granted: cerynth_agent_api::ResourceBudget,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: u32,
    pub policy: Config,
    pub claims: BTreeMap<Uuid, Claim>,
}
#[derive(Debug, Clone)]
pub struct Authority {
    directory: PathBuf,
    policy_hash: String,
}
#[derive(Debug, Serialize)]
pub struct Receipt {
    #[serde(flatten)]
    pub decision: Decision,
    pub claim: Option<Claim>,
    pub existing: bool,
    pub kernel_applied: bool,
}

fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        (if directory { m.is_dir() } else { m.is_file() })
            && m.uid() == nix::unistd::getuid().as_raw()
            && m.mode() & 0o777 == if directory { 0o700 } else { 0o600 },
        "authority path must be private, owned and non-symlink"
    );
    Ok(())
}
fn locked(directory: &Path) -> Result<File> {
    private(directory, true)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(directory.join("authority.lock"))?;
    private(&directory.join("authority.lock"), false)?;
    FileExt::try_lock_exclusive(&lock)
        .context("authority transaction busy; retry without changing request")?;
    Ok(lock)
}
fn load(directory: &Path) -> Result<State> {
    let path = directory.join("authority.json");
    private(&path, false)?;
    ensure!(
        std::fs::metadata(&path)?.len() <= 4 * 1024 * 1024,
        "authority journal too large"
    );
    let state: State = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        state.version == 1 && state.claims.len() <= 1024,
        "unsupported authority journal"
    );
    state.policy.validate()?;
    for (id, claim) in &state.claims {
        ensure!(
            *id == claim.demand.session_id
                && !id.is_nil()
                && !claim.owner.is_empty()
                && claim.demand.parent_session_id.is_none()
                && claim.granted.fits_within(&claim.demand.requested),
            "invalid authority claim"
        );
        claim.granted.validate().map_err(anyhow::Error::msg)?;
    }
    Ok(state)
}
fn store(directory: &Path, state: &State) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    ensure!(bytes.len() <= 4 * 1024 * 1024, "authority journal full");
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(directory.join("authority.json"))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}
impl Config {
    /// # Errors
    /// Rejects invalid capacities or unbounded device configuration.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.capacity.memory_bytes > 0
                && self.capacity.cpu_quota_micros_per_second >= 10_000
                && self
                    .capacity
                    .cpu_quota_micros_per_second
                    .is_multiple_of(10_000)
                && self.inference_limit <= 1024
                && self.accelerators.len() <= 16,
            "invalid authority capacity policy"
        );
        ensure!(
            self.accelerators.iter().all(|(id, a)| !id.is_empty()
                && id.len() <= 128
                && !id.chars().any(char::is_control)
                && a.total_bytes > a.desktop_reserve_bytes),
            "invalid accelerator policy"
        );
        Ok(())
    }
}
impl Authority {
    /// Explicit owner initialization; live policy replacement is forbidden.
    /// # Errors
    /// Rejects unsafe directories, existing journals and malformed policy.
    pub fn initialize(directory: &Path, policy: Config) -> Result<()> {
        policy.validate()?;
        if !directory.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
        }
        let _lock = locked(directory)?;
        ensure!(
            !directory.join("authority.json").try_exists()?,
            "authority already initialized; policy replacement requires a separate reviewed migration"
        );
        store(
            directory,
            &State {
                version: 1,
                policy,
                claims: BTreeMap::new(),
            },
        )
    }
    /// # Errors
    /// Rejects unsafe, malformed or locked journals.
    pub fn open(directory: &Path) -> Result<Self> {
        let _lock = locked(directory)?;
        let state = load(directory)?;
        Ok(Self {
            directory: directory.canonicalize()?,
            policy_hash: digest(&state.policy)?,
        })
    }
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    #[must_use]
    pub fn policy_fingerprint(&self) -> &str {
        &self.policy_hash
    }
    fn transaction<T>(&self, operation: impl FnOnce(&mut State) -> Result<(T, bool)>) -> Result<T> {
        let _lock = locked(&self.directory)?;
        let mut state = load(&self.directory)?;
        ensure!(
            digest(&state.policy)? == self.policy_hash,
            "authority policy changed during use"
        );
        let (result, changed) = operation(&mut state)?;
        if changed {
            store(&self.directory, &state)?;
        }
        Ok(result)
    }
    /// # Errors
    /// Returns an error for busy/corrupt state. No missing observation becomes capacity.
    pub fn view(&self) -> Result<State> {
        self.transaction(|state| Ok((state.clone(), false)))
    }
    /// Root reservations include their subtrees; children must use their parent's
    /// kernel hierarchy and local subset accounting rather than double reserving.
    /// # Errors
    /// Rejects ID substitution, child claims, unbounded CPU and stale pressure.
    pub fn reserve(
        &self,
        owner: &str,
        demand: &Demand,
        sample: &Sample,
        free_gpu: &BTreeMap<String, u64>,
    ) -> Result<Receipt> {
        ensure!(
            !owner.is_empty()
                && owner.len() <= 128
                && !demand.session_id.is_nil()
                && !demand.request_id.is_nil()
                && demand.parent_session_id.is_none()
                && demand.requested.cpu_quota_micros_per_second.is_some(),
            "root authority needs stable IDs and a bounded CPU quota"
        );
        sample.validate(crate::pressure::now_ms())?;
        self.transaction(|state| {
            if let Some(claim) = state.claims.get(&demand.session_id) {
                ensure!(
                    claim.owner == owner && digest(&claim.demand)? == digest(demand)?,
                    "authority session ID reused with different owner or demand"
                );
                return Ok((
                    Receipt {
                        decision: Decision::Accept {
                            granted: claim.granted.clone(),
                        },
                        claim: Some(claim.clone()),
                        existing: true,
                        kernel_applied: false,
                    },
                    false,
                ));
            }
            ensure!(state.claims.len() < 1024, "authority claim bound reached");
            let snapshot = snapshot(state, sample, free_gpu)?;
            let proposal = global::plan(demand, &snapshot, crate::pressure::now_ms())?;
            let claim = proposal.decision.granted().map(|granted| Claim {
                owner: owner.into(),
                demand: demand.clone(),
                granted: granted.clone(),
            });
            if let Some(claim) = &claim {
                state.claims.insert(demand.session_id, claim.clone());
            }
            let changed = claim.is_some();
            Ok((
                Receipt {
                    decision: proposal.decision,
                    claim,
                    existing: false,
                    kernel_applied: false,
                },
                changed,
            ))
        })
    }
    /// Trusted caller must verify the whole subtree is empty before release.
    /// No lease expiry, PID heuristic, or other member's restart releases a claim.
    /// # Errors
    /// Rejects another owner's release and preserves claims on I/O uncertainty.
    pub fn release(&self, owner: &str, session: Uuid) -> Result<()> {
        self.transaction(|state| {
            if let Some(claim) = state.claims.get(&session) {
                ensure!(
                    claim.owner == owner,
                    "cannot release another authority member's claim"
                );
            }
            let changed = state.claims.remove(&session).is_some();
            Ok(((), changed))
        })
    }
}
fn snapshot(
    state: &State,
    sample: &Sample,
    free_gpu: &BTreeMap<String, u64>,
) -> Result<OwnerSnapshot> {
    let mut reserved = Capacity {
        memory_bytes: 0,
        cpu_quota_micros_per_second: 0,
    };
    let mut inference_reserved = 0_u32;
    let mut accelerators: BTreeMap<_, _> = state
        .policy
        .accelerators
        .iter()
        .map(|(id, a)| {
            (
                id.clone(),
                AcceleratorCapacity {
                    total_bytes: a.total_bytes,
                    observed_free_bytes: free_gpu.get(id).copied(),
                    reserved_bytes: 0,
                    desktop_reserve_bytes: a.desktop_reserve_bytes,
                    cooperative_admission_supported: true,
                },
            )
        })
        .collect();
    for claim in state.claims.values() {
        reserved.memory_bytes = reserved
            .memory_bytes
            .checked_add(claim.granted.memory_max_bytes)
            .context("memory accounting overflow")?;
        reserved.cpu_quota_micros_per_second = reserved
            .cpu_quota_micros_per_second
            .checked_add(
                claim
                    .granted
                    .cpu_quota_micros_per_second
                    .context("unbounded persisted CPU claim")?,
            )
            .context("CPU accounting overflow")?;
        inference_reserved = inference_reserved
            .checked_add(claim.granted.inference_concurrency)
            .context("inference accounting overflow")?;
        if claim.granted.gpu_memory_bytes > 0 {
            let gpu = claim
                .demand
                .gpu_device_id
                .as_ref()
                .and_then(|id| accelerators.get_mut(id))
                .context("claim GPU missing from policy")?;
            gpu.reserved_bytes = gpu
                .reserved_bytes
                .checked_add(claim.granted.gpu_memory_bytes)
                .context("GPU accounting overflow")?;
        }
    }
    Ok(OwnerSnapshot {
        observed_at_ms: sample.observed_at_ms,
        complete_managed_workload_accounting: true,
        capacity: state.policy.capacity.clone(),
        reserved,
        parent_authority: None,
        accelerators,
        inference_limit: state.policy.inference_limit,
        inference_reserved,
        pressure_blocks_new_work: sample.blocks_new_work(crate::pressure::now_ms()),
    })
}
