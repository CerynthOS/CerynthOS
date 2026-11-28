use cerynth_ipc::{AdaptationMode, Profile, SchedulerBackend};
use serde::{Deserialize, Serialize};

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static SAVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Runtime state persisted by the daemon.
///
/// Unlike `Config`, this represents the current runtime state that
/// should survive daemon restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeState {
    pub profile: Profile,
    pub adaptation_enabled: bool,
    pub adaptation_mode: AdaptationMode,
    pub scheduler_backend: SchedulerBackend,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            adaptation_enabled: false,
            adaptation_mode: AdaptationMode::Off,
            scheduler_backend: SchedulerBackend::Mock,
        }
    }
}

impl RuntimeState {
    /// Read without conflating missing, unreadable and malformed metadata.
    ///
    /// # Errors
    /// Returns filesystem errors or invalid data for malformed JSON.
    pub fn try_load(path: &str) -> io::Result<Self> {
        serde_json::from_slice(&fs::read(path)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    #[must_use]
    pub fn load(path: &str) -> Self {
        if !Path::new(path).exists() {
            return Self::default();
        }

        Self::try_load(path).unwrap_or_default()
    }

    /// Persists state to `path`, reporting rather than panicking on failure.
    ///
    /// This runs on every IPC request, so an unwritable state directory must
    /// not be able to take the daemon down.
    pub fn save(&self, path: &str) {
        if let Err(error) = self.try_save(path) {
            eprintln!("warning: cannot atomically persist runtime state to {path}: {error}");
        }
    }

    /// Write, fsync and rename in one directory so readers see a complete old
    /// or new snapshot. Parent fsync makes the rename durable across reboot.
    ///
    /// # Errors
    /// Reports serialization, creation, write, rename or durability errors.
    pub fn try_save(&self, path: &str) -> io::Result<()> {
        let target = Path::new(path);
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let name = target
            .file_name()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let sequence = SAVE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{}.{}.{sequence}.tmp",
            name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(&serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
            file.sync_all()?;
            fs::rename(&temporary, target)?;
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Each test gets its own path: the tests run concurrently and a shared
    /// filename makes them race against each other.
    fn test_file(name: &str) -> String {
        format!(
            "{}/cerynth-test-state-{name}.json",
            std::env::temp_dir().display()
        )
    }

    #[test]
    fn save_and_load_state() {
        let state = RuntimeState {
            profile: Profile::Interactive,
            adaptation_enabled: true,
            adaptation_mode: AdaptationMode::Shadow,
            scheduler_backend: SchedulerBackend::Mock,
        };

        state.save(&test_file("roundtrip"));

        let loaded = RuntimeState::load(&test_file("roundtrip"));

        assert_eq!(loaded.profile, Profile::Interactive);
        assert!(loaded.adaptation_enabled);
        assert_eq!(loaded.adaptation_mode, AdaptationMode::Shadow);

        let _ = std::fs::remove_file(test_file("roundtrip"));
    }

    #[test]
    fn missing_state_returns_default() {
        let _ = std::fs::remove_file(test_file("missing"));

        let state = RuntimeState::load(&test_file("missing"));

        assert_eq!(state.profile, Profile::Balanced);
        assert!(!state.adaptation_enabled);
        assert_eq!(state.adaptation_mode, AdaptationMode::Off);
    }

    #[test]
    fn corrupt_state_returns_default() {
        std::fs::write(test_file("corrupt"), "not json").unwrap();

        let state = RuntimeState::load(&test_file("corrupt"));

        assert_eq!(state.profile, Profile::Balanced);
        assert!(!state.adaptation_enabled);
        assert_eq!(state.adaptation_mode, AdaptationMode::Off);

        let _ = std::fs::remove_file(test_file("corrupt"));
    }

    #[test]
    fn concurrent_atomic_saves_never_expose_partial_json() {
        let path = test_file("atomic");
        RuntimeState::default().try_save(&path).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        RuntimeState::default().try_save(path).unwrap();
                    }
                });
            }
            for _ in 0..200 {
                RuntimeState::try_load(&path).unwrap();
            }
        });
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_save_replaces_symlink_without_following_it() {
        let path = test_file("symlink");
        let unrelated = test_file("unrelated");
        let _ = fs::remove_file(&path);
        fs::write(&unrelated, "preserve").unwrap();
        std::os::unix::fs::symlink(&unrelated, &path).unwrap();
        RuntimeState::default().try_save(&path).unwrap();
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "preserve");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(path).unwrap();
        fs::remove_file(unrelated).unwrap();
    }
}
