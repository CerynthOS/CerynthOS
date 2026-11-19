# Supported private cache process mode

Launch the persistent inference coordinator with:

```sh
CERYNTH_CACHE_PRIVACY=managed_process cerynth-inferenced --directory /absolute/private/state serve
```

The supported mode requires a coordinator-owned Ollama backend started through
`start_backend`, with the exact registered endpoint identity and URL. Each
session and authorization domain binds a distinct live process generation.
Model files can be shared, but process memory and a new private HOME directory
are separate. This intentionally duplicates resident weights across sessions.
The default `trust_domain` mode retains its less restrictive sharing behavior.

Managed agents must use the Guard native inference bridge with host networking
denied. agentd rejects a strict-mode grant for an unsupported execution boundary
before exposing its token to the harness. Unisolated host processes and an
owner who can directly access the machine remain outside this guarantee.

Strict mode refuses external or unowned endpoints, endpoint aliases and generic
compatible engines, including vLLM/SGLang/LMCache configurations. It also refuses
client-supplied cache namespace/transfer controls. Those engines are not declared
safe merely because a cache salt or process budget is available. The mode
trusts the owner-selected native Ollama executable; it is not protection against
a malicious backend, arbitrary external offloading or hardware side channels.

Revoking a token does not reset its cache domain. Reusing an endpoint for a new
session or changed authorization requires revoking its grants and stopping the
owned backend. Only after the entire recorded systemd scope is verified empty
does the coordinator release the cache-domain binding. A subsequent backend
gets a new private HOME and process generation. Restart recovers old scopes
before admitting new work; grants do not survive restart.

Routing state version 2 persists the privacy choice. Forgetting the environment
flag on restart preserves strict mode. Explicitly requesting a downgrade of an
existing strict state directory fails. Old version 1 routing state is readable;
an older binary cannot read the new version 2 state. Preserve a stopped-state
backup before rollback, and do not share a live state directory across versions.

The real local fixture `tools/cache_privacy_smoke.py` verifies separate processes
and private homes, rejected cross-session reuse/aliases/external engines/cache
forgery, authorization reset only after scope cleanup, persistence across
restart, rejected downgrade, and absence of a public canary in another session's
response. These are enforcement checks, not statistical timing-noninterference
or a general proof of language-model confidentiality.
