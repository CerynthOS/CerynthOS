#!/usr/bin/env python3
"""Produce compatibility data with the actual previous-checkpoint Rust types."""
import hashlib,json,os,subprocess,tempfile
from pathlib import Path

REVISION='2d8b5d4'
def main():
    root=Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix='cerynth-legacy-producer-') as temporary:
        path=Path(temporary);api=path/'api';(api/'src').mkdir(parents=True);(path/'src').mkdir()
        hashes={}
        for name in ['lib.rs','runtime.rs']:
            data=subprocess.check_output(['git','show',REVISION+':crates/cerynth-agent-api/src/'+name],cwd=root)
            (api/'src'/name).write_bytes(data);hashes[name]=hashlib.sha256(data).hexdigest()
        (api/'Cargo.toml').write_text('''[package]
name="cerynth-agent-api"
version="0.1.0"
edition="2024"
[dependencies]
async-trait="0.1"
serde={version="1",features=["derive"]}
serde_json="1"
thiserror="2"
uuid={version="1",features=["v4","serde"]}
schemars={version="0.8",features=["uuid1"]}
''')
        (path/'Cargo.toml').write_text('''[package]
name="cerynth-legacy-producer"
version="0.0.0"
edition="2024"
[dependencies]
cerynth-agent-api={path="api"}
serde_json="1"
''')
        (path/'src/main.rs').write_text('''use serde_json::{Value,json};
fn main() {
 let id="11111111-1111-4111-8111-111111111111";let op="22222222-2222-4222-8222-222222222222";
 let spec=json!({"name":"public-legacy-fixture","harness":"generic_pty","workspace":"/tmp/public-legacy-fixture",
  "model":{"name":"none"},"runtime":{"guard":{"capabilities":[{"kind":"fs.read","target":"workspace"}]},
  "budget":{"cpu_quota_micros_per_second":500000}}});
 let mut requests=Vec::new();
 for operation in [json!({"type":"create","spec":spec}),json!({"type":"list"}),json!({"type":"status","id":id}),
   json!({"type":"start","id":id,"operation_id":op,"launch":{"program":"/usr/bin/true","args":[]}}),
   json!({"type":"stop","id":id}),json!({"type":"resource_admit","id":id,"minimum":null}),
   json!({"type":"resource_accounting","id":id})] {
  let old:cerynth_agent_api::Request=serde_json::from_value(json!({"version":1,"operation":operation})).unwrap();
  requests.push(serde_json::to_value(old).unwrap());
 }
 let session:cerynth_agent_api::Session=serde_json::from_value(json!({"id":id,"owner_uid":1000,"spec":spec,
  "state":"created","boundary":"unenforced_prototype","created_at_ms":0,"updated_at_ms":0,"events":[],
  "execution":{"operations":[{"id":op,"fingerprint":"legacy-public-action","status":"unknown"}]}})).unwrap();
 let output:Value=json!({"requests":requests,"session":session});
 println!("{}",serde_json::to_string_pretty(&output).unwrap());
}
''')
        result=subprocess.run([os.environ.get('CARGO',str(Path.home()/'.cargo/bin/cargo')),'run','--offline','--quiet','--manifest-path',str(path/'Cargo.toml')],
            env={**os.environ,'CARGO_TARGET_DIR':str(root/'target/legacy-contract-producer')},capture_output=True,text=True,check=True)
        value=json.loads(result.stdout);value['producer_revision']=REVISION;value['producer_source_sha256']=hashes
        destination=root/'crates/cerynth-agent-api/tests/fixtures/legacy-2d8b5d4.json';destination.parent.mkdir(parents=True,exist_ok=True)
        destination.write_text(json.dumps(value,indent=2)+'\n');print('Legacy Rust producer fixture generated')

if __name__=='__main__':main()
