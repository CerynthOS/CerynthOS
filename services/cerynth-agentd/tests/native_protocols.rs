//! Offline native-process contracts. Live `OpenCode` acceptance is an opt-in host check.
use anyhow::Result;
use std::{os::unix::fs::PermissionsExt, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

const NATIVE: &str = r"#!/usr/bin/python3
import base64, http.server, json, os, sys
def send(x): print(json.dumps(x), flush=True)
if sys.argv[1] == 'acp':
    def read(): return json.loads(sys.stdin.readline())
    req = read()
    assert req['params']['clientCapabilities'] == {}
    send({'jsonrpc':'2.0','id':req['id'],'result':{'protocolVersion':1,'agentCapabilities':{}}})
    req = read()
    assert req['params']['mcpServers'] == []
    send({'jsonrpc':'2.0','id':req['id'],'result':{'sessionId':'native-fixture'}})
    req = read()
    assert req['params']['prompt'][0]['text'] == 'private-fixture-prompt'
    send({'jsonrpc':'2.0','id':77,'method':'fs/read_text_file','params':{'path':'/host-secret'}})
    assert read()['error']['code'] == -32601
    send({'jsonrpc':'2.0','id':78,'method':'session/request_permission','params':{}})
    assert read()['result']['outcome']['outcome'] == 'cancelled'
    for i in range(300):
        send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'native-fixture',
            'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'x'}}}})
    send({'jsonrpc':'2.0','id':req['id'],'result':{'stopReason':'end_turn'}})
    sys.stdin.read()
else:
    class API(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args): pass
        def reply(self,body):
            expected='Basic '+base64.b64encode(('opencode:'+os.environ['OPENCODE_SERVER_PASSWORD']).encode()).decode()
            assert self.headers.get('authorization') == expected
            data=json.dumps(body).encode()
            self.send_response(200); self.send_header('content-length',str(len(data))); self.end_headers();self.wfile.write(data)
        def do_GET(self):
            if self.path == '/global/health': self.reply({'healthy':True,'version':'fixture'})
            elif self.path == '/session/ses_fixture/message':
                self.reply([{'info':{'role':'user'},'parts':[{'type':'text','text':'private-fixture-prompt'}]},
                    {'info':{'role':'assistant','tokens':{'input':11,'output':7}},'parts':[
                        {'type':'tool','callID':'intermediate-tool','state':{'status':'completed','metadata':{'exit':127}}}]},
                    {'info':{'role':'assistant','tokens':{'input':3,'output':2}},'parts':[{'type':'text','text':'final answer'}]}])
            else: raise AssertionError(self.path)
        def do_POST(self):
            body=json.loads(self.rfile.read(int(self.headers['content-length'])))
            if self.path == '/session': self.reply({'id':'ses_fixture'})
            elif self.path == '/session/ses_fixture/message':
                assert body['parts'][0]['text'] == 'private-fixture-prompt'
                assert body['model'] == {'providerID':'ollama','modelID':'org/model:quant'}
                self.reply({'info':{'role':'assistant'},'parts':[{'type':'text','text':'final answer'}]})
            else: raise AssertionError(self.path)
    http.server.HTTPServer(('127.0.0.1',int(sys.argv[sys.argv.index('--port')+1])),API).serve_forever()
";

async fn run(mode: &str) -> Result<String> {
    let directory = tempfile::tempdir()?;
    let native = directory.path().join("native-fixture");
    std::fs::write(&native, NATIVE)?;
    std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o700))?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_cerynth-harness-bridge"))
        .args([mode, native.to_str().unwrap(), "org/model:quant"])
        .env_clear()
        .env("HOME", directory.path())
        .env("PATH", "/usr/bin:/bin")
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"private-fixture-prompt")
        .await?;
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await??;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

#[tokio::test]
async fn http_bridge_authenticates_and_includes_intermediate_tool_history() -> Result<()> {
    let output = run("http").await?;
    assert!(output.contains("intermediate-tool"));
    assert!(output.contains("native_turn_completed"));
    assert!(!output.contains("private-fixture-prompt"));
    assert!(!output.contains("Basic "));
    Ok(())
}

#[tokio::test]
async fn acp_bridge_streams_large_turns_and_denies_ungranted_host_operations() -> Result<()> {
    let output = run("acp").await?;
    assert_eq!(
        output
            .lines()
            .filter(|line| line.contains("agent_message_chunk"))
            .count(),
        300
    );
    assert!(output.contains("native_turn_completed"));
    assert!(!output.contains("private-fixture-prompt"));
    Ok(())
}
