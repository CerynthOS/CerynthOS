# Authorization engine: Cedar prototype

Status: provisional, pending team review and a comparable OPA prototype.

Guard currently uses Cedar in process. This gives the Rust daemon a policy
engine without a separate service and maps principals, actions and resources
onto Guard's decision model. Invalid policy or evaluation errors deny access.
Authorization policy is separate from CPU and memory adaptation policy.

OPA/Rego remains a candidate: it can run as a separate policy service and has a
broader data-oriented policy model, at the cost of another service boundary and
operational dependency. No equivalent OPA prototype or performance comparison
has been performed, so a conclusive comparative decision is not claimed.
Neither engine mediates system calls; sandboxing and trusted brokers enforce
what an agent can actually do. Policy decisions must not be labeled enforced
when callers can bypass them.
