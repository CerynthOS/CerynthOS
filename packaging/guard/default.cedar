// Cerynth Guard default guardrails.
//
// These apply on top of each session's capabilities: a session can only do
// what it was granted AND what these policies allow. The stricter one wins.
//
//   principal  Cerynth::Session::"<session uuid>"
//   action     Cerynth::Action::"<capability kind>"   e.g. "git.push"
//   resource   Cerynth::Target::"<target>"
//   context    target, args, mode, origin, harness, is_child
//
// A permit annotated @mode("approval") makes matching requests wait for a
// human. Any policy that fails to evaluate denies the request.

@id("baseline")
permit (principal, action, resource);

@id("git-push-needs-approval")
@mode("approval")
permit (principal, action == Cerynth::Action::"git.push", resource);

@id("no-force-push")
forbid (principal, action == Cerynth::Action::"git.push", resource)
when { context.args has force && context.args.force == true };

@id("protect-default-branches")
forbid (principal, action == Cerynth::Action::"git.push", resource)
when { context.target like "*:refs/heads/main" || context.target like "*:refs/heads/master" };

@id("subagents-cannot-push")
forbid (principal, action == Cerynth::Action::"git.push", resource)
when { context.is_child };

@id("host-writes-need-approval")
@mode("approval")
permit (principal, action == Cerynth::Action::"fs.write", resource)
when { context.target != "workspace" };

@id("credentials-only-for-agents-not-children")
forbid (principal, action == Cerynth::Action::"credential.use", resource)
when { context.is_child && !(context.args has delegated && context.args.delegated == true) };
