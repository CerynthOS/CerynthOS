# Guard runtime and desktop integration

The service trusts its own Unix user. A host process running as that user can
use the control socket. Sandbox isolation must expose only the requesting
session's socket, never the control socket or the runtime directory. This is a
single-user trust model. The dedicated system account service is an alternative
packaging example, not a multi-user authorization implementation.

Install a per-user development instance using `scripts/guard/install-user.sh`.
It preserves existing configuration and does not activate the service.
Start using the printed command. The default config uses a private directory
under XDG_RUNTIME_DIR and stores state under ~/.local/state/cerynth/guard.

The Rust integration API is `cerynth_guardd::client::Client`. Each request has an
explicit deadline; timeout or disconnect gives no authorization. The wire API is
newline JSON defined in `protocol.rs`. `cerynth-guardctl --socket PATH status`,
`pending`, `verify-audit`, and `request FILE` exercise that same API.

Runtime sequence:

1. Create a UUID and an isolated workspace that the agent may fully modify.
2. RegisterSession on the trusted socket with GuardSession and trusted remotes.
3. Request LaunchProfile. Only a Launch response supplies the sandbox argv.
4. Launch the argv as a child of the P1 resource scope. Guard does not create
   cgroups or own the child process lifecycle.
5. Give the sandbox only its own socket. Agent-side Decide cannot approve or
   register sessions. Generic decisions are advisory; Git push is brokered.
6. Stop and reap the sandbox process tree before EndSession. End children before
   parents. Session-end is authorization cleanup, not process termination.

Desktop sequence: query PendingApprovals on the trusted socket, display the
immutable action and digest, then submit Approve with that digest. Never let an
agent submit the trusted request on behalf of the user. Subscribe provides live
events; after disconnect or a lagged stream, query current state and inspect the
audit log. Audit events can contain sensitive target metadata: restrict access.

Process contract validation:

```sh
cargo build -p cerynth-security --bins
GUARD_EXEC="$PWD/target/debug/cerynth-guard-exec" cargo test -p cerynth-guardd --test managed_execution -- --ignored
```

This validates a real daemon and sandbox. A real harness, resource scope and
Qt desktop integration require their respective implementations; none are
substituted by this contract test. Network access remains disabled until the
egress gateway is implemented and tested.
