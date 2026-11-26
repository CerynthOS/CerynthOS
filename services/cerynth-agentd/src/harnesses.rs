fn normalized_event(
    kind: &str,
    id: Option<&str>,
    usage: Option<&Value>,
) -> cerynth_agent_api::NormalizedHarnessEvent {
    cerynth_agent_api::NormalizedHarnessEvent {
        sequence: 0,
        kind: kind.into(),
        tool_call_id: id.filter(|s| s.len() <= 256).map(str::to_owned),
        input_tokens: usage
            .and_then(|u| u["input_tokens"].as_u64().or_else(|| u["input"].as_u64())),
        output_tokens: usage
            .and_then(|u| u["output_tokens"].as_u64().or_else(|| u["output"].as_u64())),
        exit_code: None,
        cached_input_tokens: usage.and_then(|u| {
            u["cache"]["read"]
                .as_u64()
                .or_else(|| u["cached_input_tokens"].as_u64())
                .or_else(|| u["input_tokens_details"]["cached_tokens"].as_u64())
                .or_else(|| u["cache_read_input_tokens"].as_u64())
        }),
        cache_write_tokens: usage.and_then(|u| {
            u["cache"]["write"]
                .as_u64()
                .or_else(|| u["cache_creation_input_tokens"].as_u64())
        }),
    }
}

use crate::AgentService;

fn normalize_acp(value: &Value) -> Vec<cerynth_agent_api::NormalizedHarnessEvent> {
    let update = &value["params"]["update"];
    let kind = match update["sessionUpdate"].as_str() {
        Some("tool_call") => "harness_tool_started",
        Some("tool_call_update") if update["status"] == "failed" => "harness_tool_failed",
        Some("tool_call_update") if update["status"] == "completed" => "harness_tool_completed",
        _ => return Vec::new(),
    };
    vec![normalized_event(kind, update["toolCallId"].as_str(), None)]
}

pub fn normalize(line: &[u8]) -> Vec<cerynth_agent_api::NormalizedHarnessEvent> {
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    if value["type"] == "acp_update" {
        return normalize_acp(&value);
    }
    let mut events = Vec::new();
    let event_type = value["type"].as_str().unwrap_or_default();
    let mut push = |kind: &str, id: Option<&str>, usage: Option<&Value>| {
        let mut event = normalized_event(kind, id, usage);
        event.exit_code = value["item"]["exit_code"]
            .as_i64()
            .or_else(|| value["part"]["state"]["metadata"]["exit"].as_i64())
            .and_then(|code| i32::try_from(code).ok());
        if event.kind == "harness_tool_completed" && event.exit_code.is_some_and(|code| code != 0) {
            event.kind = "harness_tool_failed".into();
        }
        events.push(event);
    };
    match event_type {
        "item.started" | "item.completed"
            if matches!(
                value["item"]["type"].as_str(),
                Some("command_execution" | "file_change" | "mcp_tool_call")
            ) =>
        {
            push(
                if event_type == "item.started" {
                    "harness_tool_started"
                } else {
                    "harness_tool_completed"
                },
                value["item"]["id"].as_str(),
                None,
            );
        }
        "assistant" => {
            if let Some(content) = value["message"]["content"].as_array() {
                for item in content {
                    if item["type"] == "tool_use" {
                        push("harness_tool_started", item["id"].as_str(), None);
                    }
                }
            }
        }
        "user" => {
            if let Some(content) = value["message"]["content"].as_array() {
                for item in content {
                    if item["type"] == "tool_result" {
                        push(
                            if item["is_error"] == true {
                                "harness_tool_failed"
                            } else {
                                "harness_tool_completed"
                            },
                            item["tool_use_id"].as_str(),
                            None,
                        );
                    }
                }
            }
        }
        "tool_call" => push("harness_tool_started", value["id"].as_str(), None),
        "tool_use" => push(
            if value["part"]["state"]["status"] == "error" {
                "harness_tool_failed"
            } else if value["part"]["state"]["status"] == "completed" {
                "harness_tool_completed"
            } else {
                "harness_tool_started"
            },
            value["part"]["callID"]
                .as_str()
                .or_else(|| value["tool_call_id"].as_str())
                .or_else(|| value["id"].as_str()),
            None,
        ),
        "tool_result" => push(
            if value["is_error"] == true {
                "harness_tool_failed"
            } else {
                "harness_tool_completed"
            },
            value["tool_call_id"].as_str(),
            None,
        ),
        "turn.completed" | "result" | "usage" => {
            push(
                "harness_usage_reported",
                None,
                value.get("usage").or_else(|| value.get("tokens")),
            );
        }
        "step_finish" => push("harness_usage_reported", None, value["part"].get("tokens")),
        "error" | "turn.failed" => push("harness_error_reported", None, None),
        _ => {}
    }
    events
}
use anyhow::{Result, ensure};
use cerynth_agent_api::{HarnessAdapter, HarnessKind, LaunchRequest, Session};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::io::AsyncReadExt;

pub fn executable(name: &str) -> Option<PathBuf> {
    let override_name = match name {
        "opencode" => Some("CERYNTH_OPENCODE_BINARY"),
        "hermes" => Some("CERYNTH_HERMES_BINARY"),
        "codex" => Some("CERYNTH_CODEX_BINARY"),
        "claude" => Some("CERYNTH_CLAUDE_BINARY"),
        _ => None,
    };
    if let Some(path) = override_name.and_then(std::env::var_os).map(PathBuf::from) {
        return path
            .is_absolute()
            .then(|| path.canonicalize().ok())
            .flatten()
            .filter(|p| p.is_file());
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join(name))
        .find(|path| path.is_file())
        .and_then(|p| p.canonicalize().ok())
}

pub async fn launch(session: &Session, input: String) -> Result<LaunchRequest> {
    if session.spec.runtime.transport != cerynth_agent_api::HarnessTransport::CliJsonl {
        return Ok(
            cerynth_adapter_cli_harnesses::protocol::OpenCodeProtocolAdapter {
                native_program: executable("opencode")
                    .ok_or_else(|| anyhow::anyhow!("OpenCode is not installed"))?,
                bridge_program: protocol_bridge()?,
                transport: session.spec.runtime.transport,
            }
            .prepare_launch(session, cerynth_agent_api::AdapterInput::Prompt(input))
            .await?,
        );
    }
    let name = match session.spec.harness {
        HarnessKind::OpenCode => "opencode",
        HarnessKind::Hermes => "hermes",
        HarnessKind::Codex => "codex",
        HarnessKind::ClaudeCode => "claude",
        _ => anyhow::bail!("unsupported native harness; use start for generic_pty"),
    };
    let program = executable(name).ok_or_else(|| anyhow::anyhow!("{name} is not installed"))?;
    Ok(cerynth_adapter_cli_harnesses::CliHarnessAdapter {
        kind: session.spec.harness,
        program,
    }
    .prepare_launch(session, cerynth_agent_api::AdapterInput::Prompt(input))
    .await?)
}

pub(crate) fn protocol_bridge() -> Result<PathBuf> {
    std::env::var_os("CERYNTH_HARNESS_BRIDGE")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?.with_file_name("cerynth-harness-bridge"))
        .canonicalize()
        .map_err(Into::into)
}

impl AgentService {
    pub(crate) fn bind_harness_profile(
        &self,
        session: &Session,
        plan: &mut crate::process::PreparedLaunch,
    ) -> Result<()> {
        use std::{
            io::Write,
            os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        };
        if !session.spec.runtime.hermes_terminal_profile {
            return Ok(());
        }
        plan.environment
            .push(("HERMES_IGNORE_RULES".into(), "1".into()));
        ensure!(
            session.spec.harness == HarnessKind::Hermes
                && session.spec.runtime.isolation == cerynth_agent_api::IsolationPolicy::Required
                && session.spec.runtime.guard.is_none(),
            "Hermes terminal profile requires the supported isolated CLI boundary"
        );
        let directory = self.directory.join("harness-profiles");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
        let path = directory.join(format!("{}.yaml", session.id));
        let content = b"tools:\n  tool_search:\n    enabled: 'off'\nterminal:\n  backend: local\nplugins:\n  enabled: []\n  auto_update_check_hours: 0\nhooks: {}\nmcp_servers: {}\n";
        if path.exists() {
            let metadata = std::fs::symlink_metadata(&path)?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == session.owner_uid
                    && metadata.mode() & 0o777 == 0o600
                    && std::fs::read(&path)? == content,
                "harness profile changed"
            );
        } else {
            let mut file = tempfile::NamedTempFile::new_in(&directory)?;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
            file.write_all(content)?;
            file.as_file().sync_all()?;
            file.persist_noclobber(&path)?;
        }
        let bwrap = plan
            .args
            .iter()
            .position(|s| s == "/usr/bin/bwrap")
            .ok_or_else(|| anyhow::anyhow!("sandbox missing"))?;
        let boundary = plan
            .args
            .iter()
            .enumerate()
            .skip(bwrap + 1)
            .find(|(_, s)| s.as_str() == "--")
            .map(|(i, _)| i)
            .ok_or_else(|| anyhow::anyhow!("sandbox payload boundary missing"))?;
        plan.args.splice(
            boundary..boundary,
            [
                "--dir".into(),
                "/home/agent/.hermes".into(),
                "--ro-bind".into(),
                path.to_string_lossy().into_owned(),
                "/home/agent/.hermes/config.yaml".into(),
            ],
        );
        Ok(())
    }
    pub(crate) async fn discover_harness(&self, harness: HarnessKind) -> Result<Value> {
        ensure!(
            harness == HarnessKind::OpenCode,
            "native session discovery is unsupported for this harness"
        );
        let client = self
            .native_opencode
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("owner has not configured CERYNTH_OPENCODE_URL"))?;
        client.health().await?;
        let native = client.sessions().await?;
        let sessions = native
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("invalid native session list"))?;
        ensure!(
            sessions.len() <= 128,
            "native session discovery exceeds 128 entries"
        );
        let discovered:Vec<_>=sessions.iter().filter(|session|session["id"].as_str().is_some_and(|id|
            !id.is_empty() && id.len()<=256 && id.chars().all(|c|c.is_ascii_alphanumeric() || c=='_' || c=='-')))
            .map(|session|json!({"native_id":session["id"],"title":session["title"].as_str().map(|s|s.chars().take(256).collect::<String>()),
                "directory":session["directory"].as_str().filter(|s|s.len()<=4096),"parent_id":session["parentID"].as_str().filter(|s|s.len()<=256),
                "source":"harness_report","boundary":"unenforced_prototype","imported":false})).collect();
        Ok(json!({"harness":"open_code","sessions":discovered,"control_acquired":false}))
    }

    pub(crate) async fn adapter_status(&self) -> Result<Value> {
        let mut descriptors = vec![json!(
            cerynth_adapter_generic_pty::GenericPtyAdapter.descriptor()
        )];
        for name in ["opencode", "hermes", "codex", "claude"] {
            let path = executable(name);
            let authentication = if let Some(path) = &path {
                authentication_state(name, path).await
            } else {
                "not_installed"
            };
            let version = if let Some(path) = &path {
                installation_version(path).await
            } else {
                "not_installed".into()
            };
            let kind = match name {
                "opencode" => HarnessKind::OpenCode,
                "hermes" => HarnessKind::Hermes,
                "codex" => HarnessKind::Codex,
                _ => HarnessKind::ClaudeCode,
            };
            let mut capabilities = path.as_ref().map_or_else(Vec::new, |program| {
                cerynth_adapter_cli_harnesses::CliHarnessAdapter {
                    kind,
                    program: program.clone(),
                }
                .descriptor()
                .capabilities
            });
            if name == "opencode" && self.native_opencode.is_some() {
                capabilities.push(cerynth_agent_api::AdapterCapability::Discovery);
            }
            let protocol_transports =
                if name == "opencode" && path.is_some() && protocol_bridge().is_ok() {
                    vec!["open_code_http", "open_code_acp"]
                } else {
                    Vec::new()
                };
            descriptors.push(
                json!({"name":name,"path":path,"installed":path.is_some(),"version":version,
                "transport":"supervised_cli_jsonl","capabilities":capabilities,
                "optional_transports":protocol_transports,
                "authentication":authentication,"native_pause_resume":false,
                "authentication_scope":"owner profile inventory; managed sessions do not inherit it",
                "saved_inventory_only":matches!(name,"opencode"|"hermes"),
                "cloud_credentials_inherited":false}),
            );
        }
        Ok(json!(descriptors))
    }
}

async fn installation_version(path: &std::path::Path) -> String {
    let Ok(home) = tempfile::tempdir() else {
        return "unavailable".into();
    };
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(path)
            .arg("--version")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", home.path())
            .env("HERMES_HOME", home.path().join(".hermes"))
            .env("DO_NOT_TRACK", "1")
            .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
            .env("OPENCODE_DISABLE_MODELS_FETCH", "true")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    match output {
        Ok(Ok(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(128)
            .collect(),
        _ => "unavailable".into(),
    }
}

async fn authentication_state(name: &str, path: &std::path::Path) -> &'static str {
    let arguments: &[&str] = match name {
        "codex" => &["login", "status"],
        "claude" => &["auth", "status", "--json"],
        "opencode" | "hermes" => &["auth", "list"],
        _ => return "unsupported",
    };
    let output = tokio::time::timeout(Duration::from_secs(5), async {
        // Run local inventory outside the repository. Output is bounded and
        // reduced to an enum; provider/account labels never reach the caller.
        let directory = tempfile::tempdir()?;
        let mut child = tokio::process::Command::new(path)
            .args(arguments)
            .current_dir(directory.path())
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .envs(
                [
                    "HOME",
                    "XDG_CONFIG_HOME",
                    "XDG_DATA_HOME",
                    "XDG_STATE_HOME",
                    "HERMES_HOME",
                ]
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
            )
            .env("OPENCODE_DISABLE_MODELS_FETCH", "true")
            .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("auth probe stdout missing"))?
            .take(32769)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(bytes.len() <= 32768, "auth inventory output exceeds limit");
        Ok::<_, anyhow::Error>((child.wait().await?.success(), bytes))
    })
    .await;
    let Ok(Ok((success, bytes))) = output else {
        return "unknown";
    };
    if name == "claude" {
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        return match value["loggedIn"].as_bool() {
            Some(true) => "authenticated",
            Some(false) => "unauthenticated",
            None => "unknown",
        };
    }
    if matches!(name, "opencode" | "hermes") {
        return local_auth_inventory(name, success, &bytes);
    }
    if success { "authenticated" } else { "unknown" }
}

fn local_auth_inventory(name: &str, success: bool, bytes: &[u8]) -> &'static str {
    if !success {
        return "unknown";
    }
    let text = String::from_utf8_lossy(bytes);
    // Read only the native count marker. Do not confuse a valid configured key
    // with a live provider authentication/entitlement check.
    for line in text.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        for pair in words.windows(2) {
            if pair[1].starts_with("credential") {
                if let Ok(count) = pair[0].trim_start_matches('(').parse::<u32>() {
                    if count > 0 {
                        return "credentials_configured";
                    }
                    if name == "opencode" {
                        return "no_saved_credentials";
                    }
                }
            }
        }
    }
    if name == "hermes" && text.trim().is_empty() {
        return "no_saved_credentials";
    }
    "unknown"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_binding_rejects_tampering_and_keeps_configuration_outside_workspace() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir()?;
        let workspace = tempfile::tempdir()?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let service = AgentService::open(directory.path(), "http://127.0.0.1:11434")?;
        let mut registry =
            crate::registry::Registry::open(&directory.path().join("fixture-registry"))?;
        let session = registry.create(serde_json::from_value(
            json!({"name":"profile","harness":"hermes","workspace":workspace.path(),
            "model":{"name":"fixture"},"runtime":{"hermes_terminal_profile":true}}),
        )?)?;
        let mut plan = crate::process::PreparedLaunch {
            program: "/usr/bin/systemd-run".into(),
            args: vec![
                "--".into(),
                "/usr/bin/bwrap".into(),
                "--dir".into(),
                "/home/agent".into(),
                "--".into(),
                "/usr/bin/hermes".into(),
            ],
            environment: vec![],
            unit: None,
        };
        service.bind_harness_profile(&session, &mut plan)?;
        let path = directory
            .path()
            .join("harness-profiles")
            .join(format!("{}.yaml", session.id));
        assert!(!path.starts_with(workspace.path()));
        assert!(std::fs::read_to_string(&path)?.contains("enabled: 'off'"));
        assert!(
            plan.args
                .iter()
                .any(|a| a == "/home/agent/.hermes/config.yaml")
        );
        assert!(
            plan.environment
                .iter()
                .any(|(k, v)| k == "HERMES_IGNORE_RULES" && v == "1")
        );
        std::fs::write(&path, b"changed")?;
        assert!(service.bind_harness_profile(&session, &mut plan).is_err());
        Ok(())
    }

    #[test]
    fn authentication_inventory_exposes_only_state_and_does_not_validate_keys_online() {
        assert_eq!(
            local_auth_inventory("opencode", true, b"\x1b[90m\n 0 credentials\n"),
            "no_saved_credentials"
        );
        assert_eq!(
            local_auth_inventory(
                "opencode",
                true,
                b"provider private@example.invalid\n 1 credential\n"
            ),
            "credentials_configured"
        );
        assert_eq!(
            local_auth_inventory(
                "hermes",
                true,
                b"private-provider (2 credentials):\n #1 key=secret-canary"
            ),
            "credentials_configured"
        );
        assert_eq!(
            local_auth_inventory("hermes", true, b""),
            "no_saved_credentials"
        );
        assert_eq!(
            local_auth_inventory("opencode", false, b"1 credential"),
            "unknown"
        );
        assert_eq!(
            local_auth_inventory("hermes", true, b"unexpected output secret-canary"),
            "unknown"
        );
    }

    #[test]
    fn native_tool_failures_ids_and_usage_remain_distinct_without_raw_content() {
        let acp = normalize(br#"{"type":"acp_update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"acp-tool","status":"failed","rawOutput":{"token":"secret"}}}}"#);
        assert_eq!(acp[0].kind, "harness_tool_failed");
        assert_eq!(acp[0].tool_call_id.as_deref(), Some("acp-tool"));
        assert!(!serde_json::to_string(&acp).unwrap().contains("secret"));
        let failed=normalize(br#"{"type":"tool_use","part":{"callID":"call_native","state":{"status":"error","input":{"api_key":"secret"},"error":"private command"}}}"#);
        assert_eq!(failed[0].kind, "harness_tool_failed");
        assert_eq!(failed[0].tool_call_id.as_deref(), Some("call_native"));
        assert!(!serde_json::to_string(&failed).unwrap().contains("secret"));
        let hermes =
            normalize(br#"{"type":"tool_use","tool_call_id":"call_hermes","name":"tool_call"}"#);
        assert_eq!(hermes[0].tool_call_id.as_deref(), Some("call_hermes"));
        let result =
            normalize(br#"{"type":"tool_result","tool_call_id":"call_hermes","is_error":true}"#);
        assert_eq!(result[0].kind, "harness_tool_failed");
        let usage=normalize(br#"{"type":"step_finish","part":{"tokens":{"input":11,"output":7,"cache":{"read":13}}}}"#);
        assert_eq!(usage[0].input_tokens, Some(11));
        assert_eq!(usage[0].output_tokens, Some(7));
        assert_eq!(usage[0].cached_input_tokens, Some(13));
        let command=normalize(br#"{"type":"item.completed","item":{"id":"cmd","type":"command_execution","aggregated_output":"private"}}"#);
        assert_eq!(command[0].tool_call_id.as_deref(), Some("cmd"));
        assert!(normalize(b"unstructured terminal output").is_empty());
        let command=normalize(br#"{"type":"item.completed","item":{"id":"failed-command","type":"command_execution","exit_code":127}}"#);
        assert_eq!(command[0].kind, "harness_tool_failed");
        assert_eq!(command[0].exit_code, Some(127));
    }
}
