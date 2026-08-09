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
- Turn exits: 0 / 0
- Raw stream digests: sha256:caa158e08a33e4ea9058d2170789e67fb9d4e9fce9058970e861ad1ced766266 / sha256:7f00215b9ac9fe6a20bf992e2f39c34e2b67a70bc9541e126242cef2479a06a3
- Normalized artifact: docs/spikes/SPIKE-002/events/codex.jsonl (sha256:b1d8cda7ea608f91c07b616a0e63c658cfebcd122405abe950c96bb34073f8a0)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: noninteractive bounded file-edit permission flags; no URL or unrestricted path permission granted
- Native event types: item.completed, item.started, thread.started, turn.completed, turn.started
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "--json", "--sandbox", "workspace-write", "--skip-git-repo-check", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "resume", "019fe8dc-e1cf-7e73-a63b-a8558ad3af05", "--json", "<redacted-prompt>"]

### claude

- Executable: /Users/alvaro/.local/share/claude/versions/2.1.220
- Version: 2.1.220 (Claude Code)
- Result: PASS — all required capabilities observed
- Working-directory token/digest: $PROBE_ROOT/claude / sha256:74fb55ad22f8f9a179213ba67cdda59696bfb58f5d06c08886b02f9c484fe2e4
- Observed/resumed session: fdf28489-7639-44fa-957b-b4c46c078e50 / fdf28489-7639-44fa-957b-b4c46c078e50
- Turn exits: 0 / 0
- Raw stream digests: sha256:365d7b451ba79ac78803de38b70f9a56fc4a03df52e713807c33d8ed51aca931 / sha256:7f97df57361a696505de22d441d6ef8ab12cbf6e4c398cde8891614499e32d71
- Normalized artifact: docs/spikes/SPIKE-002/events/claude.jsonl (sha256:0bc38c2a7a3809f2586a8256ec90ea22b67bef6ce8984be92749a4e804927cb6)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: noninteractive bounded file-edit permission flags; no URL or unrestricted path permission granted
- Native event types: assistant, rate_limit_event, result, system, user
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--session-id", "fdf28489-7639-44fa-957b-b4c46c078e50", "<redacted-prompt>"]
- Turn-two argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--resume", "fdf28489-7639-44fa-957b-b4c46c078e50", "<redacted-prompt>"]

### copilot

- Executable: /opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot
- Version: GitHub Copilot CLI 1.0.78.
- Result: PASS — all required capabilities observed
- Working-directory token/digest: $PROBE_ROOT/copilot / sha256:8697a4a7b3a03af9827e6365acd123a0bd5a9856ba65a02e31b3087301c6283f
- Observed/resumed session: c4af64a8-622e-48ad-8d43-41b186343286 / c4af64a8-622e-48ad-8d43-41b186343286
- Turn exits: 0 / 0
- Raw stream digests: sha256:b74a72aa6c382f60c983e1bb541f29307501c81a056f7d4f8b6f46922d476b8a / sha256:b630b819e0eaadb6ff3510f87fba97f7f19dc778ae72e51b5894c5387cb804dc
- Normalized artifact: docs/spikes/SPIKE-002/events/copilot.jsonl (sha256:4acdad3506bdd6c60155ed20748f67c0a860a0abdaf48ed5d9b94cef832197a0)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: noninteractive bounded file-edit permission flags; no URL or unrestricted path permission granted
- Native event types: assistant.idle, assistant.message, assistant.message_delta, assistant.message_start, assistant.reasoning, assistant.reasoning_delta, assistant.tool_call_delta, assistant.turn_end, assistant.turn_start, mcp.tools.list_changed, model.call_start, result, session.background_tasks_changed, session.mcp_server_status_changed, session.skills_loaded, session.tools_updated, session.usage_checkpoint, tool.execution_complete, tool.execution_partial_result, tool.execution_start, user.message
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--session-id", "c4af64a8-622e-48ad-8d43-41b186343286", "--prompt", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--resume=c4af64a8-622e-48ad-8d43-41b186343286", "--prompt", "<redacted-prompt>"]

## Custody and limitations

Only normalized metadata is committed. Raw stdout and stderr were hashed and deleted from probe scratch storage after normalization.
No prompt, free-form provider output, reasoning, credential, environment dump, or raw scratch path is retained.
Provider-native event mapping is version-sensitive; unavailable fields are listed per provider.
