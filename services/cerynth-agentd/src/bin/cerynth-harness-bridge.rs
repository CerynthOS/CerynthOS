//! Owned native protocol driver. The supervisor supplies the execution boundary.
use anyhow::{Context, Result, ensure};
use cerynth_agent_protocols::{AcpClient, opencode::OpenCodeClient};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, Command},
};

fn emit(value: &Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

async fn stop(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn http(program: &PathBuf, model: &str, prompt: &str) -> Result<()> {
    let reserved = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = reserved.local_addr()?.port();
    let origin = format!("http://127.0.0.1:{port}");
    let password = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let client = OpenCodeClient::with_password(&origin, Some(password.clone()))?;
    drop(reserved);
    let mut child = Command::new(program)
        .args([
            "serve",
            "--pure",
            "--hostname",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .env("OPENCODE_SERVER_PASSWORD", password)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let result = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(Ok(health)) = tokio::time::timeout(Duration::from_millis(500), client.health()).await {
                ensure!(health["healthy"] == true, "OpenCode API is unhealthy");
                break;
            }
            ensure!(child.try_wait()?.is_none(), "OpenCode API exited during startup");
            ensure!(tokio::time::Instant::now() < deadline, "OpenCode API readiness deadline exceeded");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        let created = client.create_session("Cerynth managed task").await?;
        let id = created["id"].as_str().context("OpenCode returned no session ID")?;
        emit(&json!({"type":"native_session_started","transport":"open_code_http","native_id":id}))?;
        let response = client.prompt(id, "ollama", model, prompt).await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let _ = tokio::time::timeout(Duration::from_secs(2), client.cancel(id)).await;
                return Err(error);
            }
        };
        ensure!(response["info"]["error"].is_null(), "OpenCode API reported a failed turn");
        // Read back from the supported history endpoint; no inference replay.
        let history = client.messages(id).await?;
        let messages = history.as_array().context("invalid OpenCode history response")?;
        ensure!(messages.len() <= 256, "OpenCode history exceeds limit");
        // A prompt response can contain only the final assistant message. Tool
        // calls from intermediate turns must come from the session history.
        for message in messages.iter().filter(|m| m["info"]["role"] == "assistant") {
            for part in message["parts"].as_array().context("invalid OpenCode message parts")? {
                emit(&json!({"type":if part["type"] == "tool" {"tool_use"} else {"native_part"},"part":part}))?;
            }
            emit(&json!({"type":"step_finish","part":{"tokens":message["info"]["tokens"]}}))?;
        }
        emit(&json!({"type":"native_turn_completed","transport":"open_code_http","history_messages":history.as_array().map_or(0,Vec::len)}))?;
        Ok(())
    }.await;
    stop(&mut child).await;
    result
}

async fn acp(program: &PathBuf, prompt: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(["acp", "--pure"])
        // OpenCode ACP may also open an internal HTTP listener. Protect it as
        // well; the stdio peer never needs to see this ephemeral credential.
        .env("OPENCODE_SERVER_PASSWORD", format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let result = async {
        let mut client = AcpClient::new(BufReader::new(child.stdout.take().context("ACP stdout unavailable")?),
            child.stdin.take().context("ACP stdin unavailable")?);
        client.initialize().await?;
        client.create_session(std::env::current_dir()?.to_str().context("non-UTF8 workspace")?).await?;
        emit(&json!({"type":"native_session_started","transport":"open_code_acp","native_id":client.session_id,
            "client_filesystem":false,"client_terminal":false,"permission_policy":"deny"}))?;
        let response = client.prompt_streaming(prompt, &mut |params| emit(&json!({"type":"acp_update","params":params}))).await;
        let response = match response {
            Ok(response) => response,
            Err(error) => { let _ = client.cancel().await; return Err(error); }
        };
        ensure!(response["stopReason"] == "end_turn", "ACP turn did not complete normally");
        emit(&json!({"type":"native_turn_completed","transport":"open_code_acp","stop_reason":response["stopReason"]}))?;
        Ok(())
    }.await;
    stop(&mut child).await;
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3,
        "usage: cerynth-harness-bridge http|acp ABSOLUTE_OPENCODE MODEL"
    );
    let program = PathBuf::from(&args[1]);
    ensure!(
        program.is_absolute() && program.is_file(),
        "invalid native executable"
    );
    ensure!(
        !args[2].is_empty() && args[2].len() <= 256 && !args[2].chars().any(char::is_control),
        "invalid model"
    );
    let mut prompt = String::new();
    tokio::io::stdin()
        .take(32769)
        .read_to_string(&mut prompt)
        .await?;
    ensure!(
        !prompt.trim().is_empty() && prompt.len() <= 32768,
        "invalid prompt"
    );
    match args[0].as_str() {
        "http" => http(&program, &args[2], &prompt).await,
        "acp" => acp(&program, &prompt).await,
        _ => anyhow::bail!("unsupported native transport"),
    }
}
