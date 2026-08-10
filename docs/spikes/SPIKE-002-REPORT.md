# SPIKE-002 Installed Provider CLI Feasibility Report

This report records a bounded two-turn disposable-worktree experiment. It does not claim a production adapter or coordinator.

| Provider | Result | Version | Exact-session resume | Expected file | Reason |
| --- | --- | --- | --- | --- | --- |
| codex | PASS | codex-cli 0.146.0 | True | True | all required capabilities observed |
| claude | PASS | 2.1.220 (Claude Code) | True | True | all required capabilities observed |
| copilot | PASS | GitHub Copilot CLI 1.0.78. | True | True | all required capabilities observed |

## Task contract

- Schema: harness.provider-feasibility-task/v1
- Version: 1
- Turn-one prompt digest: sha256:3bfa17dee1eeba5751c7068cb611c6a4bec8dd82744fbece05d02616d4d94240
- Turn-two prompt digest: sha256:885655f9feb06d83781f204608386db7042cb82dfe0b58a5743dfa1f130155b6
- Expected-final digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Task contract digest: sha256:da3d3877763a3471e229ed18ea230fdb7af1b326a1601c25eaea0f7ebb6cd231

## Evidence

### codex

- Executable: /opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin
- Version: codex-cli 0.146.0
- Result: PASS — all required capabilities observed
- Task contract digest: sha256:da3d3877763a3471e229ed18ea230fdb7af1b326a1601c25eaea0f7ebb6cd231
- Turn-one prompt digest: sha256:3bfa17dee1eeba5751c7068cb611c6a4bec8dd82744fbece05d02616d4d94240
- Turn-two prompt digest: sha256:885655f9feb06d83781f204608386db7042cb82dfe0b58a5743dfa1f130155b6
- Working-directory token/digest: $PROBE_ROOT/codex / sha256:e5cf14fff7cc10a44a687af11e119258235f0aacaf73ef11f12959b1f312460f
- Observed/resumed session: 019fe9b6-054b-7d50-b88c-d15fe177ae84 / 019fe9b6-054b-7d50-b88c-d15fe177ae84
- Turn-two observed session: 019fe9b6-054b-7d50-b88c-d15fe177ae84
- Turn exits: 0 / 0
- Raw stream digests: sha256:99c06091f5b48c90dd5e3172ba7b5ac8d805dfb745c7d669dd220622f91c2278 / sha256:ba11d2136dc8242278beb024a4105a993f9c10415b6d9b66c5e1df399a00231a
- Normalized artifact: docs/spikes/SPIKE-002/events/codex.jsonl (sha256:a52fb5284eb623915a5a6339c47510549a3f03a6e22918d8aaa384ae652a1c07)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Codex turn one used --sandbox workspace-write; the exact-session resume used --json, and neither turn used a sandbox-bypass or approval-bypass flag.
- Native event types: item.completed, item.started, thread.started, turn.completed, turn.started
- Exact native-to-normalized mapping: {"item.completed": "provider.activity", "item.started": "provider.activity", "thread.started": "session.started", "turn.completed": "turn.completed", "turn.started": "turn.started"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "--json", "--sandbox", "workspace-write", "--skip-git-repo-check", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/codex/0.146.0/codex-aarch64-apple-darwin", "exec", "resume", "019fe9b6-054b-7d50-b88c-d15fe177ae84", "--json", "<redacted-prompt>"]

### claude

- Executable: /Users/alvaro/.local/share/claude/versions/2.1.220
- Version: 2.1.220 (Claude Code)
- Result: PASS — all required capabilities observed
- Task contract digest: sha256:da3d3877763a3471e229ed18ea230fdb7af1b326a1601c25eaea0f7ebb6cd231
- Turn-one prompt digest: sha256:3bfa17dee1eeba5751c7068cb611c6a4bec8dd82744fbece05d02616d4d94240
- Turn-two prompt digest: sha256:885655f9feb06d83781f204608386db7042cb82dfe0b58a5743dfa1f130155b6
- Working-directory token/digest: $PROBE_ROOT/claude / sha256:4a500fa51eb8ff1a7b29078644bb592662f7dfe4ecdfe4302494dab43a3eb0ba
- Observed/resumed session: 6f95a601-47fd-4125-8926-8b5c89b42510 / 6f95a601-47fd-4125-8926-8b5c89b42510
- Turn-two observed session: 6f95a601-47fd-4125-8926-8b5c89b42510
- Turn exits: 0 / 0
- Raw stream digests: sha256:c97032a6dc1e7588ed3d40f2d44e86c43b636c7de73fadb659d8c7bc121c46de / sha256:a481ee01276403f91f0d12d856c635bd8733103f60c215e85e0399a85aacabd9
- Normalized artifact: docs/spikes/SPIKE-002/events/claude.jsonl (sha256:ff42bef9f786aea267dc0e9fa400bb2a95febf0b19b22ea5050a224a4239e882)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Claude used --safe-mode and --permission-mode acceptEdits on both turns; neither turn used a permission-bypass flag.
- Native event types: assistant, rate_limit_event, result, system.init, system.thinking_tokens, user
- Exact native-to-normalized mapping: {"assistant": "provider.event", "rate_limit_event": "provider.event", "result": "turn.completed", "system.init": "session.started", "system.thinking_tokens": "provider.event", "user": "provider.event"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--session-id", "6f95a601-47fd-4125-8926-8b5c89b42510", "<redacted-prompt>"]
- Turn-two argv: ["/Users/alvaro/.local/share/claude/versions/2.1.220", "--print", "--output-format", "stream-json", "--verbose", "--safe-mode", "--permission-mode", "acceptEdits", "--resume", "6f95a601-47fd-4125-8926-8b5c89b42510", "<redacted-prompt>"]

### copilot

- Executable: /opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot
- Version: GitHub Copilot CLI 1.0.78.
- Result: PASS — all required capabilities observed
- Task contract digest: sha256:da3d3877763a3471e229ed18ea230fdb7af1b326a1601c25eaea0f7ebb6cd231
- Turn-one prompt digest: sha256:3bfa17dee1eeba5751c7068cb611c6a4bec8dd82744fbece05d02616d4d94240
- Turn-two prompt digest: sha256:885655f9feb06d83781f204608386db7042cb82dfe0b58a5743dfa1f130155b6
- Working-directory token/digest: $PROBE_ROOT/copilot / sha256:25ada636910cab4c53416ce99d22b2cafef155a4c27c357f836fa3cd8166c54d
- Observed/resumed session: d7edd944-6641-46c1-bd17-eb1b4d688dc8 / d7edd944-6641-46c1-bd17-eb1b4d688dc8
- Turn-two observed session: d7edd944-6641-46c1-bd17-eb1b4d688dc8
- Turn exits: 0 / 0
- Raw stream digests: sha256:efe433e4eb811eb4dc140da8ee63bcabcef06d9bfb43b81a388dd7d8a05bef6a / sha256:565968646ac7d48d3504dcde86e99f06c203f0b990173d03be7cc0c6411afd30
- Normalized artifact: docs/spikes/SPIKE-002/events/copilot.jsonl (sha256:6c5b86bf1dee8db2b30f2d32b231a34b1e13a9e898dd17a17b41d13180bed66f)
- Final file digest: sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d
- Permission behavior: Copilot used --allow-all-tools, which the installed CLI required for noninteractive mode: all tools were auto-approved. --allow-all-paths, --allow-all-urls, --allow-all, and --yolo were absent, so path and URL verification were not disabled. The local same-user process is not a security boundary.
- Native event types: assistant.idle, assistant.message, assistant.message_delta, assistant.message_start, assistant.reasoning, assistant.reasoning_delta, assistant.tool_call_delta, assistant.turn_end, assistant.turn_start, mcp.tools.list_changed, model.call_start, result, session.info, session.mcp_server_status_changed, session.skills_loaded, session.tools_updated, session.usage_checkpoint, tool.execution_complete, tool.execution_start, user.message
- Exact native-to-normalized mapping: {"assistant.idle": "provider.event", "assistant.message": "provider.event", "assistant.message_delta": "provider.event", "assistant.message_start": "provider.event", "assistant.reasoning": "provider.event", "assistant.reasoning_delta": "provider.event", "assistant.tool_call_delta": "provider.activity", "assistant.turn_end": "turn.completed", "assistant.turn_start": "turn.started", "mcp.tools.list_changed": "provider.activity", "model.call_start": "provider.activity", "result": "turn.completed", "session.info": "provider.event", "session.mcp_server_status_changed": "provider.event", "session.skills_loaded": "provider.event", "session.tools_updated": "provider.activity", "session.usage_checkpoint": "provider.event", "tool.execution_complete": "provider.activity", "tool.execution_start": "provider.activity", "user.message": "provider.event"}
- Unavailable/unstable fields: provider-side authoritative worktree identity, provider-side cryptographic termination receipt
- Turn-one argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--session-id", "d7edd944-6641-46c1-bd17-eb1b4d688dc8", "--prompt", "<redacted-prompt>"]
- Turn-two argv: ["/opt/homebrew/Caskroom/copilot-cli/1.0.54/copilot", "--output-format", "json", "--allow-all-tools", "--resume=d7edd944-6641-46c1-bd17-eb1b4d688dc8", "--prompt", "<redacted-prompt>"]

## Custody and limitations

Only normalized metadata is committed. Raw stdout and stderr were hashed and deleted from probe scratch storage after normalization.
No prompt, free-form provider output, reasoning, credential, environment dump, or raw scratch path is retained.
Provider-native event mapping is version-sensitive; unavailable fields are listed per provider.
