# SPIKE-002 Installed Provider CLI Feasibility Report

This report records a bounded two-turn disposable-worktree experiment. It does not claim a production adapter or coordinator.

| Provider | Result | Version | Exact-session resume | Expected file | Reason |
| --- | --- | --- | --- | --- | --- |
| codex | PASS | codex-cli 0.146.0 | True | True | all required capabilities observed |
| claude | PASS | 2.1.220 (Claude Code) | True | True | all required capabilities observed |
| copilot | PASS | GitHub Copilot CLI 1.0.78. | True | True | all required capabilities observed |

## Evidence

### codex

- Executable: /opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin
- Version: codex-cli 0.146.0
- Result: PASS — all required capabilities observed
- Working-directory token/digest: $PROBE_ROOT/codex / sha256:a9b58176ff846cf128ca36855f5217c5fcbb9f78f26a1df18d9604ad6a885a4e
- Observed/resumed session: 019fe8dc-e1cf-7e73-a63b-a8558ad3af05 / 019fe8dc-e1cf-7e73-a63b-a8558ad3af05
- Turn-two observed session: 019fe8dc-e1cf-7e73-a63b-a8558ad3af05
- Turn exits: 0 / 0
- Raw stream digests: sha256:caa158e08a33e4ea9058d2170789e67fb9d4e9fce9058970e861ad1ced766266 / sha256:7f00215b9ac9fe6a20bf992e2f39c34e2b67a70bc9541e126242cef2479a06a3
- Normalized artifact: docs/spikes/SPIKE-002/events/codex.jsonl (sha256:3bd5e0877f2eaf59ef026b8996526527a86ec804a43ae1572817db4e79467472)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Codex turn one used --sandbox workspace-write; the exact-session resume used --json, and neither turn used a sandbox-bypass or approval-bypass flag.
- Native event types: item.completed, item.started, thread.started, turn.completed, turn.started
- Exact native-to-normalized mapping: {"item.completed": "provider.activity", "item.started": "provider.activity", "thread.started": "session.started", "turn.completed": "turn.completed", "turn.started": "turn.started"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "--json", "--sandbox", "workspace-write", "--skip-git-repo-check", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "resume", "019fe8dc-e1cf-7e73-a63b-a8558ad3af05", "--json", "<redacted-prompt>"]

### claude

- Executable: /Users/alvaro/.local/share/claude/versions/2.1.220
- Version: 2.1.220 (Claude Code)
- Result: PASS — all required capabilities observed
- Working-directory token/digest: $PROBE_ROOT/claude / sha256:e311b6511f98eb12c0859be39759cf98d7f1ff3d2e572e85d32d8a3b70f74970
- Observed/resumed session: 7f025a20-7eec-4245-9019-b1ddfde0bb8a / 7f025a20-7eec-4245-9019-b1ddfde0bb8a
- Turn-two observed session: 7f025a20-7eec-4245-9019-b1ddfde0bb8a
- Turn exits: 0 / 0
- Raw stream digests: sha256:519db349a32e6858ab4c4b022e77c05a8b3a2365780c19a2975e93e93e47b922 / sha256:bfc9864ae422ac6e2b0f71f6ec567fc09b133171c8114c06dba337683a6576e1
- Normalized artifact: docs/spikes/SPIKE-002/events/claude.jsonl (sha256:bb010632131791bc04b00fc0ab6a385983b51ec41a0ecc17d8981d37bb5ee07b)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Claude used --safe-mode and --permission-mode acceptEdits on both turns; neither turn used a permission-bypass flag.
- Native event types: assistant, rate_limit_event, result, system.init, system.thinking_tokens, user
- Exact native-to-normalized mapping: {"assistant": "provider.event", "rate_limit_event": "provider.event", "result": "turn.completed", "system.init": "session.started", "system.thinking_tokens": "provider.event", "user": "provider.event"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--session-id", "7f025a20-7eec-4245-9019-b1ddfde0bb8a", "<redacted-prompt>"]
- Turn-two argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--resume", "7f025a20-7eec-4245-9019-b1ddfde0bb8a", "<redacted-prompt>"]

### copilot

- Executable: /opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot
- Version: GitHub Copilot CLI 1.0.78.
- Result: PASS — all required capabilities observed
- Working-directory token/digest: $PROBE_ROOT/copilot / sha256:8697a4a7b3a03af9827e6365acd123a0bd5a9856ba65a02e31b3087301c6283f
- Observed/resumed session: c4af64a8-622e-48ad-8d43-41b186343286 / c4af64a8-622e-48ad-8d43-41b186343286
- Turn-two observed session: c4af64a8-622e-48ad-8d43-41b186343286
- Turn exits: 0 / 0
- Raw stream digests: sha256:b74a72aa6c382f60c983e1bb541f29307501c81a056f7d4f8b6f46922d476b8a / sha256:b630b819e0eaadb6ff3510f87fba97f7f19dc778ae72e51b5894c5387cb804dc
- Normalized artifact: docs/spikes/SPIKE-002/events/copilot.jsonl (sha256:69de5a846cce86a17c308b0ffb29629b727d2540bd9b725b6f93771035026ab8)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Copilot used --allow-all-tools, which the installed CLI required for noninteractive mode: all tools were auto-approved. --allow-all-paths, --allow-all-urls, --allow-all, and --yolo were absent, so path and URL verification were not disabled. The local same-user process is not a security boundary.
- Native event types: assistant.idle, assistant.message, assistant.message_delta, assistant.message_start, assistant.reasoning, assistant.reasoning_delta, assistant.tool_call_delta, assistant.turn_end, assistant.turn_start, mcp.tools.list_changed, model.call_start, result, session.background_tasks_changed, session.mcp_server_status_changed, session.skills_loaded, session.tools_updated, session.usage_checkpoint, tool.execution_complete, tool.execution_partial_result, tool.execution_start, user.message
- Exact native-to-normalized mapping: {"assistant.idle": "provider.event", "assistant.message": "provider.event", "assistant.message_delta": "provider.event", "assistant.message_start": "provider.event", "assistant.reasoning": "provider.event", "assistant.reasoning_delta": "provider.event", "assistant.tool_call_delta": "provider.activity", "assistant.turn_end": "turn.completed", "assistant.turn_start": "turn.started", "mcp.tools.list_changed": "provider.activity", "model.call_start": "provider.activity", "result": "turn.completed", "session.background_tasks_changed": "provider.event", "session.mcp_server_status_changed": "provider.event", "session.skills_loaded": "provider.event", "session.tools_updated": "provider.activity", "session.usage_checkpoint": "provider.event", "tool.execution_complete": "provider.activity", "tool.execution_partial_result": "provider.activity", "tool.execution_start": "provider.activity", "user.message": "provider.event"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--session-id", "c4af64a8-622e-48ad-8d43-41b186343286", "--prompt", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--resume=c4af64a8-622e-48ad-8d43-41b186343286", "--prompt", "<redacted-prompt>"]

## Custody and limitations

Only normalized metadata is committed. Raw stdout and stderr were hashed and deleted from probe scratch storage after normalization.
No prompt, free-form provider output, reasoning, credential, environment dump, or raw scratch path is retained.
Provider-native event mapping is version-sensitive; unavailable fields are listed per provider.
