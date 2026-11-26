# Durable Guard tool effects

For a managed Guard session, agentd replaces only the pinned Guard profile's
`/run/agent/guard.sock` bind source with a session-bound journal gateway. The
gateway forwards to the actual Guard session socket; it cannot register
sessions, approve actions or call trusted owner operations. The control socket
and sibling gateways are excluded from agent filesystem grants.

Every `decide` must carry the Guard schema, the session UUID, an operation UUID
in `request_id`, and immutable action details. Before contacting Guard, agentd
fsyncs the intent and SHA-256 fingerprint into its private journal. Concurrent
duplicates receive the existing receipt. Reusing an ID for different content or
another session fails. If the daemon or transport fails after submission, the
outcome stays unknown and the operation is never automatically resubmitted.
Known receipts survive daemon restart. Retain this journal with the registry;
deleting it removes the deduplication history. The bounded journal refuses new
operations when full rather than dropping old claims.

`await_approval` is a read of an approval already bound to this session's
recorded action. Human approval still goes through Guard's trusted owner API,
bound to the exact approval digest. The completed receipt is recorded against
the original tool operation. A recorded receipt may be a denial or pending
approval; it is not automatically a successful external effect.

Owner operations are `tool_decide {id, request}`, `tool_effects {id}` and
`tool_await_approval {id, approval_id}` in the existing versioned request frame.
Agent-side frames preserve the pinned Guard `decide`/`await_approval` protocol.
Owner-configured `runtime.guard.remotes` maps names to exact Git remote targets;
agents cannot introduce a new remote or widen an inherited remote mapping.

This provides at-most-once submission for supported Guard-brokered effects.
It does not promise exactly-once delivery, automatic rollback, or deduplication
of arbitrary shell effects outside the broker. An unknown outcome requires
owner reconciliation; a fresh UUID must not be used to blindly retry it.

`tools/tool_effects_smoke.py` exercises a real sandbox, pinned Guard and a
temporary local bare Git remote. A hook independently counts one actual push
across duplicates, concurrent calls and daemon restart. Rust fault tests also
simulate a completed external action whose acknowledgement is lost and verify
that journal recovery never submits it again. No cloud remote is used.
