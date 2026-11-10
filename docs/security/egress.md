# Session inference gateway

Guard serves each session on its private `<UUID>.egress.sock`. The launch profile
binds only this socket into the sandbox. The existing confined forwarder exposes
it on sandbox loopback port 8099. The network namespace remains isolated.

Configure trusted endpoints in guard.toml (origin URL, no path/query/userinfo):

```toml
[endpoints.local]
base_url = "http://127.0.0.1:11434/"

[endpoints.cloud]
base_url = "https://api.openai.com/"
credential = "openai"
```

Grant `inference.use` with target `local` or `cloud`. For a credential-backed
endpoint additionally grant `credential.use` for its configured credential name.
Approval-required grants receive denial at the gateway; the gateway never
silently converts an approval requirement into access.

Configure the harness SDK base URL to `http://127.0.0.1:8099/local/v1`, or
`http://127.0.0.1:8099/cloud/v1`. For an Anthropic-compatible client use
`http://127.0.0.1:8099/cloud` as the origin. Harness-specific variables must be
set by the adapter. Only clients with configurable base URLs are supported.
This is a fixed inference API gateway, not a general forward/CONNECT proxy.

A vault credential file (0600 in a 0700 directory owned by Guard) contains:

```json
{"kind":"http_header","header":"authorization","value":"Bearer EXAMPLE","base_url":"https://api.openai.com/"}
```

Set `vault_dir` in guard.toml. The credential URL must exactly equal the endpoint
URL. Only Authorization, X-Api-Key or Api-Key injection is accepted. Incoming
agent credentials, cookies, Host and forwarding headers are never copied.
Configured upstreams are trusted to handle credentials and not echo them in
response bodies. No live provider credentials were used during validation.

Only GET/POST and explicit inference/model-list paths are accepted. Upstream
redirects are blocked. HTTP is allowed only for explicitly configured loopback
hosts; other origins require validated HTTPS. No environment proxy is inherited.
Requests are limited to 8 MiB, upstream connections to 10 seconds and requests
to 120 seconds. Streaming responses use Axum/reqwest streaming. Audit intent is
written before sending upstream; bodies and credentials are not logged.

Ending a session removes its gateway socket and denies subsequent requests.
A response already being streamed may finish: cancellation of in-flight provider
work and provider charges requires provider-specific support. Host administrators
and the owning Unix user remain trusted. General domain egress, arbitrary web
browsing, OAuth/subscription auth, provider token minting and full harness
compatibility require separate adapters.

Implementation references: Axum's UnixListener support
(https://docs.rs/axum/0.8.9/axum/serve/trait.Listener.html) and reqwest's redirect
policy (https://docs.rs/reqwest/0.12.28/reqwest/redirect/struct.Policy.html).
