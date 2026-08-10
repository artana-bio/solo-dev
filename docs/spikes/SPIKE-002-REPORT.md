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
- Normalized artifact: docs/spikes/SPIKE-002/events/codex.jsonl (sha256:c763bc05be335b29b1d151608c3d560268b5041059cadbd7bbe7d00d16a3b78d)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Codex turn one used --sandbox workspace-write; the exact-session resume used --json, and neither turn used a sandbox-bypass or approval-bypass flag.
- Native event types: item.completed, item.started, thread.started, turn.completed, turn.started
- Exact native-to-normalized mapping: {"item.completed": "provider.activity", "item.started": "provider.activity", "thread.started": "session.started", "turn.completed": "turn.completed", "turn.started": "session.started"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "--json", "--sandbox", "workspace-write", "--skip-git-repo-check", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "resume", "019fe8dc-e1cf-7e73-a63b-a8558ad3af05", "--json", "<redacted-prompt>"]

### claude

- Executable: /Users/alvaro/.local/share/claude/versions/2.1.220
- Version: 2.1.220 (Claude Code)
- Result: PASS — all required capabilities observed
- Working-directory token/digest: $PROBE_ROOT/claude / sha256:74fb55ad22f8f9a179213ba67cdda59696bfb58f5d06c08886b02f9c484fe2e4
- Observed/resumed session: fdf28489-7639-44fa-957b-b4c46c078e50 / fdf28489-7639-44fa-957b-b4c46c078e50
- Turn-two observed session: fdf28489-7639-44fa-957b-b4c46c078e50
- Turn exits: 0 / 0
- Raw stream digests: sha256:365d7b451ba79ac78803de38b70f9a56fc4a03df52e713807c33d8ed51aca931 / sha256:7f97df57361a696505de22d441d6ef8ab12cbf6e4c398cde8891614499e32d71
- Normalized artifact: docs/spikes/SPIKE-002/events/claude.jsonl (sha256:0bc38c2a7a3809f2586a8256ec90ea22b67bef6ce8984be92749a4e804927cb6)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Claude used --safe-mode and --permission-mode acceptEdits on both turns; neither turn used a permission-bypass flag.
- Native event types: assistant, rate_limit_event, result, system, user
- Exact native-to-normalized mapping: {"assistant": "provider.event", "rate_limit_event": "provider.event", "result": "turn.completed", "system": "session.started", "user": "provider.event"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--session-id", "fdf28489-7639-44fa-957b-b4c46c078e50", "<redacted-prompt>"]
- Turn-two argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--resume", "fdf28489-7639-44fa-957b-b4c46c078e50", "<redacted-prompt>"]

### copilot

- Executable: /opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot
- Version: GitHub Copilot CLI 1.0.78.
- Result: PASS — all required capabilities observed
- Working-directory token/digest: $PROBE_ROOT/copilot / sha256:8697a4a7b3a03af9827e6365acd123a0bd5a9856ba65a02e31b3087301c6283f
- Observed/resumed session: c4af64a8-622e-48ad-8d43-41b186343286 / c4af64a8-622e-48ad-8d43-41b186343286
- Turn-two observed session: c4af64a8-622e-48ad-8d43-41b186343286
- Turn exits: 0 / 0
- Raw stream digests: sha256:b74a72aa6c382f60c983e1bb541f29307501c81a056f7d4f8b6f46922d476b8a / sha256:b630b819e0eaadb6ff3510f87fba97f7f19dc778ae72e51b5894c5387cb804dc
- Normalized artifact: docs/spikes/SPIKE-002/events/copilot.jsonl (sha256:88535c46cba0f4162987b85f70569fd35a738b78fa68c7f8b87a7596eeab0b0d)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Copilot used --allow-all-tools, which the installed CLI required for noninteractive mode: all tools were auto-approved. --allow-all-paths, --allow-all-urls, --allow-all, and --yolo were absent, so path and URL verification were not disabled. The local same-user process is not a security boundary.
- Native event types: assistant.idle, assistant.message, assistant.message_delta, assistant.message_start, assistant.reasoning, assistant.reasoning_delta, assistant.tool_call_delta, assistant.turn_end, assistant.turn_start, mcp.tools.list_changed, model.call_start, result, session.background_tasks_changed, session.mcp_server_status_changed, session.skills_loaded, session.tools_updated, session.usage_checkpoint, tool.execution_complete, tool.execution_partial_result, tool.execution_start, user.message
- Exact native-to-normalized mapping: {"assistant.idle": "provider.event", "assistant.message": "provider.event", "assistant.message_delta": "provider.event", "assistant.message_start": "session.started", "assistant.reasoning": "provider.event", "assistant.reasoning_delta": "provider.event", "assistant.tool_call_delta": "provider.activity", "assistant.turn_end": "turn.completed", "assistant.turn_start": "session.started", "mcp.tools.list_changed": "provider.activity", "model.call_start": "session.started", "result": "turn.completed", "session.background_tasks_changed": "provider.event", "session.mcp_server_status_changed": "provider.event", "session.skills_loaded": "provider.event", "session.tools_updated": "provider.activity", "session.usage_checkpoint": "provider.event", "tool.execution_complete": "provider.activity", "tool.execution_partial_result": "provider.activity", "tool.execution_start": "provider.activity", "user.message": "provider.event"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--session-id", "c4af64a8-622e-48ad-8d43-41b186343286", "--prompt", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--resume=c4af64a8-622e-48ad-8d43-41b186343286", "--prompt", "<redacted-prompt>"]

## Custody and limitations

Only normalized metadata is committed. Raw stdout and stderr were hashed and deleted from probe scratch storage after normalization.
No prompt, free-form provider output, reasoning, credential, environment dump, or raw scratch path is retained.
Provider-native event mapping is version-sensitive; unavailable fields are listed per provider.
