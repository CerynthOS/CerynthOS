# Pinned platform resource mechanism

This is Person 1's Rust resource mechanism from CerynthOS commit
`648c149be39f5ea8df612b3266857472976238eb`, discovered on
`origin/week1/scx-backend`. Its five Rust source files are unchanged; their
hashes and origin are recorded in [PROVENANCE.json](PROVENANCE.json).

The Cargo package is renamed from `cerynth-resources` to
`cerynth-platform-resources` to coexist with the existing Person 3 orchestration
crate. This imports no scheduler changes and claims no teammate approval.

Person 3's `cerynth-resources::platform` converts session budgets to this actual
`ResourceBudget` v1, validates them and reads typed `ResourceUsage` through
`SessionCgroup::open(...).usage()`. The existing systemd hierarchy remains the
single writer. The direct `SessionCgroup::apply` mechanism is deliberately not
used by the adapter.

The accompanying [upstream contract](../../docs/contracts/resource-budget-v1.md)
is preserved as source documentation. Its upstream CLI/integration-test paths
describe that revision; those executables are not included in this renamed
library package. Run the included original unit tests with:

```sh
cargo test --locked -p cerynth-platform-resources
```

The original source's two `assert!(!...is_empty())` test assertions trigger the
newer toolchain's `clippy::assert_is_empty` lint under `-D warnings`. Source is
preserved rather than silently rewriting Person 1's tests. Strict linting of the
eight Person 3 packages includes the pinned production library as a dependency;
the original platform tests are executed separately and in the workspace suite.
