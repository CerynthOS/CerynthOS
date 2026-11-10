//! Capabilities: typed grants attached to a session.
//!
//! See `docs/contracts/security-decision-v1alpha1.md`. Rules implemented here:
//!
//! - deny by default: an action no capability covers gets no grant at all;
//! - `deny` beats `approval` beats `allow` when several capabilities match;
//! - a child session's capabilities must be a subset of its parent's.

use serde::{Deserialize, Serialize};

/// The kinds of thing a session can be granted. Unknown kinds deserialize to
/// [`CapabilityKind::Unknown`] and never grant anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapabilityKind {
    #[serde(rename = "fs.read")]
    FsRead,
    #[serde(rename = "fs.write")]
    FsWrite,
    #[serde(rename = "net.egress")]
    NetEgress,
    #[serde(rename = "git.push")]
    GitPush,
    #[serde(rename = "credential.use")]
    CredentialUse,
    #[serde(rename = "session.spawn")]
    SessionSpawn,
    #[serde(rename = "tool.exec")]
    ToolExec,
    #[serde(rename = "inference.use")]
    InferenceUse,
    #[serde(rename = "gpu.use")]
    GpuUse,
    #[serde(other)]
    Unknown,
}

impl CapabilityKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FsRead => "fs.read",
            Self::FsWrite => "fs.write",
            Self::NetEgress => "net.egress",
            Self::GitPush => "git.push",
            Self::CredentialUse => "credential.use",
            Self::SessionSpawn => "session.spawn",
            Self::ToolExec => "tool.exec",
            Self::InferenceUse => "inference.use",
            Self::GpuUse => "gpu.use",
            Self::Unknown => "unknown",
        }
    }
}

/// How a matching capability lets the action through.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Ordered from most to least permissive, so `max()` is the strictest.
    #[default]
    Allow,
    Approval,
    Deny,
}

/// One grant. `target` is an exact string or a prefix ending in `*`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capability {
    pub kind: CapabilityKind,
    pub target: String,
    #[serde(default)]
    pub mode: Mode,
    /// Only used by `session.spawn`: how many children may exist at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Capability {
    #[must_use]
    pub fn new(kind: CapabilityKind, target: impl Into<String>, mode: Mode) -> Self {
        Self {
            kind,
            target: target.into(),
            mode,
            limit: None,
        }
    }

    #[must_use]
    pub fn matches(&self, kind: CapabilityKind, target: &str) -> bool {
        self.kind == kind
            && kind != CapabilityKind::Unknown
            && pattern_matches(&self.target, target)
    }
}

/// `pattern` is either exact or `prefix*`. A lone `*` matches everything.
#[must_use]
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => value.starts_with(prefix),
        None => pattern == value,
    }
}

/// Does `outer` match every value `inner` can match?
#[must_use]
pub fn pattern_covers(outer: &str, inner: &str) -> bool {
    match (outer.strip_suffix('*'), inner.strip_suffix('*')) {
        (Some(o), Some(i)) => i.starts_with(o),
        (Some(o), None) => inner.starts_with(o),
        (None, Some(_)) => false,
        (None, None) => outer == inner,
    }
}

/// The capabilities of one session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(pub Vec<Capability>);

impl CapabilitySet {
    /// The strictest mode among matching capabilities, or `None` if nothing
    /// matches (which callers must treat as deny).
    #[must_use]
    pub fn grant(&self, kind: CapabilityKind, target: &str) -> Option<Mode> {
        self.0
            .iter()
            .filter(|c| c.matches(kind, target))
            .map(|c| c.mode)
            .max()
    }

    pub fn of_kind(&self, kind: CapabilityKind) -> impl Iterator<Item = &Capability> {
        self.0
            .iter()
            .filter(move |c| c.kind == kind && c.mode != Mode::Deny)
    }

    #[must_use]
    pub fn spawn_limit(&self) -> u32 {
        self.of_kind(CapabilityKind::SessionSpawn)
            .map(|c| c.limit.unwrap_or(0))
            .max()
            .unwrap_or(0)
    }

    /// Check that `child` asks for nothing `self` (the parent) doesn't have.
    ///
    /// # Errors
    /// Returns the first child capability that exceeds the parent.
    pub fn check_subset(&self, child: &CapabilitySet) -> Result<(), Capability> {
        for c in &child.0 {
            if c.mode == Mode::Deny {
                continue; // a child may always restrict itself further
            }
            if c.kind == CapabilityKind::Unknown {
                return Err(c.clone());
            }
            // Any stricter parent rule overlapping the child rule must remain
            // effective. A broad allow must not erase a narrow approval rule.
            let parent_stricter = self.0.iter().any(|p| {
                p.kind == c.kind
                    && p.mode > c.mode
                    && (pattern_covers(&p.target, &c.target)
                        || pattern_covers(&c.target, &p.target))
            });
            let covered = self.0.iter().any(|p| {
                p.kind == c.kind
                    && p.mode != Mode::Deny
                    && p.mode <= c.mode
                    && pattern_covers(&p.target, &c.target)
                    && match c.kind {
                        CapabilityKind::SessionSpawn => {
                            c.limit.unwrap_or(0) <= p.limit.unwrap_or(0)
                        }
                        _ => true,
                    }
            });
            if parent_stricter || !covered {
                return Err(c.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CapabilityKind as K;

    fn cap(kind: K, target: &str, mode: Mode) -> Capability {
        Capability::new(kind, target, mode)
    }

    #[test]
    fn patterns() {
        assert!(pattern_matches(
            "origin:refs/heads/agent/*",
            "origin:refs/heads/agent/fix"
        ));
        assert!(!pattern_matches(
            "origin:refs/heads/agent/*",
            "origin:refs/heads/main"
        ));
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("exact", "exact"));
        assert!(!pattern_matches("exact", "exactly"));
        assert!(pattern_covers("a*", "ab*"));
        assert!(pattern_covers("a*", "abc"));
        assert!(!pattern_covers("ab*", "a*"));
        assert!(!pattern_covers("abc", "a*"));
    }

    #[test]
    fn deny_by_default_and_strictest_wins() {
        let set = CapabilitySet(vec![
            cap(K::GitPush, "origin:*", Mode::Allow),
            cap(K::GitPush, "origin:refs/heads/main", Mode::Deny),
            cap(K::GitPush, "origin:refs/heads/release/*", Mode::Approval),
        ]);
        assert_eq!(
            set.grant(K::GitPush, "origin:refs/heads/feature"),
            Some(Mode::Allow)
        );
        assert_eq!(
            set.grant(K::GitPush, "origin:refs/heads/main"),
            Some(Mode::Deny)
        );
        assert_eq!(
            set.grant(K::GitPush, "origin:refs/heads/release/1"),
            Some(Mode::Approval)
        );
        assert_eq!(set.grant(K::GitPush, "upstream:refs/heads/x"), None);
        assert_eq!(set.grant(K::NetEgress, "example.com:443"), None);
    }

    #[test]
    fn unknown_kind_never_grants() {
        let set: CapabilitySet =
            serde_json::from_str(r#"[{"kind":"root.everything","target":"*"}]"#).unwrap();
        assert_eq!(set.0[0].kind, K::Unknown);
        assert_eq!(set.grant(K::Unknown, "x"), None);
    }

    #[test]
    fn child_must_be_subset_of_parent() {
        let parent = CapabilitySet(vec![
            cap(K::FsWrite, "workspace", Mode::Allow),
            cap(K::NetEgress, "api.anthropic.com:443", Mode::Allow),
            cap(K::GitPush, "origin:refs/heads/agent/*", Mode::Approval),
            Capability {
                limit: Some(2),
                ..cap(K::SessionSpawn, "child", Mode::Allow)
            },
        ]);
        // Fine: narrower or equal.
        let ok = CapabilitySet(vec![
            cap(K::FsWrite, "workspace", Mode::Allow),
            cap(K::GitPush, "origin:refs/heads/agent/sub/*", Mode::Approval),
            cap(K::NetEgress, "evil.example:443", Mode::Deny), // self-restriction
        ]);
        assert!(parent.check_subset(&ok).is_ok());

        // Broader target.
        let wider = CapabilitySet(vec![cap(K::GitPush, "origin:*", Mode::Approval)]);
        assert!(parent.check_subset(&wider).is_err());
        // Approval downgraded to allow.
        let looser = CapabilitySet(vec![cap(
            K::GitPush,
            "origin:refs/heads/agent/x",
            Mode::Allow,
        )]);
        assert!(parent.check_subset(&looser).is_err());
        // Kind the parent doesn't have.
        let other = CapabilitySet(vec![cap(K::GpuUse, "*", Mode::Allow)]);
        assert!(parent.check_subset(&other).is_err());
        // Spawn limit above the parent's.
        let spawn = CapabilitySet(vec![Capability {
            limit: Some(5),
            ..cap(K::SessionSpawn, "child", Mode::Allow)
        }]);
        assert!(parent.check_subset(&spawn).is_err());
    }

    #[test]
    fn parent_deny_cannot_be_reopened_by_child() {
        let parent = CapabilitySet(vec![
            cap(K::GitPush, "origin:*", Mode::Allow),
            cap(K::GitPush, "origin:refs/heads/main", Mode::Deny),
        ]);
        let child = CapabilitySet(vec![cap(K::GitPush, "origin:refs/heads/*", Mode::Allow)]);
        assert!(parent.check_subset(&child).is_err());
    }

    #[test]
    fn parent_approval_cannot_be_downgraded_by_child() {
        let parent = CapabilitySet(vec![
            cap(K::GitPush, "origin:*", Mode::Allow),
            cap(K::GitPush, "origin:refs/heads/release/*", Mode::Approval),
        ]);
        let broad = CapabilitySet(vec![cap(K::GitPush, "origin:*", Mode::Allow)]);
        let exact = CapabilitySet(vec![cap(
            K::GitPush,
            "origin:refs/heads/release/v1",
            Mode::Allow,
        )]);
        let approved = CapabilitySet(vec![cap(K::GitPush, "origin:*", Mode::Approval)]);
        assert!(parent.check_subset(&broad).is_err());
        assert!(parent.check_subset(&exact).is_err());
        assert!(parent.check_subset(&approved).is_ok());
    }

    #[test]
    fn json_shape_matches_contract() {
        let json = r#"[{ "kind": "git.push", "target": "origin:refs/heads/agent/*", "mode": "approval" },
                       { "kind": "session.spawn", "target": "child", "limit": 2 }]"#;
        let set: CapabilitySet = serde_json::from_str(json).unwrap();
        assert_eq!(set.0[0].mode, Mode::Approval);
        assert_eq!(set.0[1].mode, Mode::Allow);
        assert_eq!(set.spawn_limit(), 2);
    }
}
