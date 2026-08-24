#!/usr/bin/env python3
"""Run one bounded provider CLI feasibility probe and retain metadata only."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import uuid


SCHEMA = "harness.provider-feasibility-report/v1"
EVENT_SCHEMA = "harness.provider-feasibility-event/v1"
PROVIDERS = ("codex", "claude", "copilot")
SCRIPT_DIR = Path(__file__).resolve().parent
RESULTS_PATH = SCRIPT_DIR / "results.json"
EVENT_DIR = SCRIPT_DIR / "events"
REPORT_PATH = SCRIPT_DIR.parent / "SPIKE-002-REPORT.md"
SCRATCH_ROOT = Path(
    os.environ.get(
        "SPIKE002_SCRATCH_ROOT",
        str(Path(tempfile.gettempdir()) / "spike002-provider-probe"),
    )
)
EXPECTED_BYTES = b"turn-one\nORBIT-2719\n"
TASK_CONTRACT_SCHEMA = "harness.provider-feasibility-task/v1"
TASK_CONTRACT_VERSION = 1
CANONICAL_TURN_ONE_PROMPT_SHA256 = "sha256:3bfa17dee1eeba5751c7068cb611c6a4bec8dd82744fbece05d02616d4d94240"
CANONICAL_TURN_TWO_PROMPT_SHA256 = "sha256:885655f9feb06d83781f204608386db7042cb82dfe0b58a5743dfa1f130155b6"
EXPECTED_PASS_UNAVAILABLE_FIELDS = (
    "provider-side authoritative worktree identity",
    "provider-side cryptographic termination receipt",
)
PERMISSION_BEHAVIORS = {
    "codex": (
        "Codex turn one used --sandbox workspace-write; the exact-session resume used --json, "
        "and neither turn used a sandbox-bypass or approval-bypass flag."
    ),
    "claude": (
        "Claude used --safe-mode and --permission-mode acceptEdits on both turns; "
        "neither turn used a permission-bypass flag."
    ),
    "copilot": (
        "Copilot used --allow-all-tools, which the installed CLI required for noninteractive mode: "
        "all tools were auto-approved. --allow-all-paths, --allow-all-urls, --allow-all, and --yolo "
        "were absent, so path and URL verification were not disabled. The local same-user process "
        "is not a security boundary."
    ),
}
NORMALIZED_EVENT_TYPES = {
    "codex": {
        "item.completed": "provider.activity",
        "item.started": "provider.activity",
        "thread.started": "session.started",
        "turn.completed": "turn.completed",
        "turn.started": "turn.started",
    },
    "claude": {
        "assistant": "provider.event",
        "rate_limit_event": "provider.event",
        "result": "turn.completed",
        "system.init": "session.started",
        "system.thinking_tokens": "provider.event",
        "user": "provider.event",
    },
    "copilot": {
        "assistant.idle": "provider.event",
        "assistant.message": "provider.event",
        "assistant.message_delta": "provider.event",
        "assistant.message_start": "provider.event",
        "assistant.reasoning": "provider.event",
        "assistant.reasoning_delta": "provider.event",
        "assistant.tool_call_delta": "provider.activity",
        "assistant.turn_end": "turn.completed",
        "assistant.turn_start": "turn.started",
        "mcp.tools.list_changed": "provider.activity",
        "model.call_start": "provider.activity",
        "result": "turn.completed",
        "session.background_tasks_changed": "provider.event",
        "session.info": "provider.event",
        "session.mcp_server_status_changed": "provider.event",
        "session.skills_loaded": "provider.event",
        "session.tools_updated": "provider.activity",
        "session.usage_checkpoint": "provider.event",
        "tool.execution_complete": "provider.activity",
        "tool.execution_partial_result": "provider.activity",
        "tool.execution_start": "provider.activity",
        "user.message": "provider.event",
    },
}
SESSION_KEYS = {
    "thread_id",
    "threadId",
    "session_id",
    "sessionId",
    "conversation_id",
    "conversationId",
}
TYPE_KEYS = ("type", "event_type", "eventType", "kind")


def sha256_bytes(value: bytes) -> str:
    return "sha256:" + hashlib.sha256(value).hexdigest()


def sha256_text(value: str) -> str:
    return sha256_bytes(value.encode("utf-8"))


def task_contract() -> dict[str, object]:
    contract: dict[str, object] = {
        "schema": TASK_CONTRACT_SCHEMA,
        "version": TASK_CONTRACT_VERSION,
        "turn_one_prompt_sha256": CANONICAL_TURN_ONE_PROMPT_SHA256,
        "turn_two_prompt_sha256": CANONICAL_TURN_TWO_PROMPT_SHA256,
        "expected_final_sha256": sha256_bytes(EXPECTED_BYTES),
    }
    contract["task_contract_sha256"] = sha256_bytes(
        json.dumps(contract, sort_keys=True, separators=(",", ":")).encode("utf-8")
    )
    return contract


def prompt_digest_field(turn: int) -> str:
    if turn == 1:
        return "turn_one_prompt_sha256"
    if turn == 2:
        return "turn_two_prompt_sha256"
    raise ValueError(f"invalid probe turn: {turn}")


def read_verified_prompt(root: Path, turn: int, expected_digest: str) -> tuple[str, str]:
    prompt_digest_field(turn)
    path = root / f"turn-{'one' if turn == 1 else 'two'}.input"
    try:
        raw = path.read_bytes()
    except OSError as exc:
        raise RuntimeError(f"canonical turn-{turn} prompt input is unavailable") from exc
    actual_digest = sha256_bytes(raw)
    if actual_digest != expected_digest:
        raise RuntimeError(f"canonical turn-{turn} prompt digest mismatch")
    try:
        return raw.decode("utf-8"), actual_digest
    except UnicodeDecodeError as exc:
        raise RuntimeError(f"canonical turn-{turn} prompt is not valid UTF-8") from exc


def verified_task_inputs(root: Path = SCRATCH_ROOT) -> tuple[dict[str, object], tuple[str, str], tuple[str, str]]:
    contract = task_contract()
    one = read_verified_prompt(root, 1, str(contract["turn_one_prompt_sha256"]))
    two = read_verified_prompt(root, 2, str(contract["turn_two_prompt_sha256"]))
    return contract, one, two


def require_results_task_contract(results: dict[str, object]) -> dict[str, object]:
    contract = task_contract()
    if results.get("task_contract") != contract:
        raise RuntimeError("results task contract does not match the fixed canonical task")
    return contract


def require_provider_task_binding(row: dict[str, object], contract: dict[str, object]) -> None:
    expected_contract = contract["task_contract_sha256"]
    if row.get("task_contract_sha256") != expected_contract:
        raise RuntimeError("provider row task contract does not match results")
    for turn, row_field in ((1, "turn_one"), (2, "turn_two")):
        expected_prompt = contract[prompt_digest_field(turn)]
        if row.get(prompt_digest_field(turn)) != expected_prompt:
            raise RuntimeError(f"provider row turn-{turn} prompt digest does not match results")
        turn_row = row.get(row_field)
        if not isinstance(turn_row, dict):
            raise RuntimeError(f"provider row {row_field} is missing")
        if turn_row.get("task_contract_sha256") != expected_contract:
            raise RuntimeError(f"provider {row_field} task contract does not match results")
        if turn_row.get("prompt_sha256") != expected_prompt:
            raise RuntimeError(f"provider {row_field} prompt digest does not match results")


def merge_provider_row(results: dict[str, object], provider: str, row: dict[str, object]) -> None:
    contract = require_results_task_contract(results)
    require_provider_task_binding(row, contract)
    providers = results.get("providers")
    if not isinstance(providers, dict):
        raise RuntimeError("results provider matrix is not an object")
    for existing in providers.values():
        if not isinstance(existing, dict):
            raise RuntimeError("existing provider row is not an object")
        require_provider_task_binding(existing, contract)
    providers[provider] = row


def run_checked(argv: list[str], cwd: Path) -> bytes:
    result = subprocess.run(argv, cwd=cwd, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return result.stdout


def minimal_environment() -> dict[str, str]:
    allowed = {
        "PATH",
        "HOME",
        "USER",
        "TMPDIR",
        "SHELL",
        "LANG",
        "LC_ALL",
        "TERM",
        "COLORTERM",
        "XDG_CONFIG_HOME",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "GH_CONFIG_DIR",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "COPILOT_GITHUB_TOKEN",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
    }
    return {key: value for key, value in os.environ.items() if key in allowed}


def executable_for(provider: str) -> str:
    name = "claude" if provider == "claude" else provider
    resolved = shutil.which(name)
    if not resolved:
        raise RuntimeError(f"installed executable not found: {name}")
    return str(Path(resolved).resolve())


def version_argv(provider: str, executable: str) -> list[str]:
    return [executable, "--version"]


def help_argv(provider: str, executable: str) -> list[str]:
    if provider == "codex":
        return [executable, "exec", "--help"]
    return [executable, "--help"]


def provider_argv(provider: str, executable: str, prompt: str, session: str | None, turn: int) -> list[str]:
    if provider == "codex":
        if turn == 1:
            return [
                executable,
                "exec",
                "--json",
                "--sandbox",
                "workspace-write",
                "--skip-git-repo-check",
                prompt,
            ]
        assert session
        return [executable, "exec", "resume", session, "--json", prompt]
    if provider == "claude":
        common = [
            executable,
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--safe-mode",
            "--permission-mode",
            "acceptEdits",
        ]
        if turn == 1:
            assert session
            return common + ["--session-id", session, prompt]
        assert session
        return common + ["--resume", session, prompt]
    if provider == "copilot":
        common = [executable, "--output-format", "json", "--allow-all-tools"]
        if turn == 1:
            assert session
            return common + ["--session-id", session, "--prompt", prompt]
        assert session
        return common + [f"--resume={session}", "--prompt", prompt]
    raise ValueError(provider)


def sanitize_argv(argv: list[str], prompt: str) -> list[str]:
    return ["<redacted-prompt>" if item == prompt else item for item in argv]


def create_disposable_repository(provider: str) -> Path:
    SCRATCH_ROOT.mkdir(parents=True, exist_ok=True)
    root = (SCRATCH_ROOT / provider).resolve()
    if root.parent != SCRATCH_ROOT.resolve():
        raise RuntimeError("unsafe scratch path")
    if root.exists():
        shutil.rmtree(root)
    root.mkdir()
    (root / "seed.txt").write_text("provider-neutral probe\n", encoding="utf-8")
    run_checked(["git", "init", "--quiet"], root)
    run_checked(["git", "config", "user.name", "SPIKE-002"], root)
    run_checked(["git", "config", "user.email", "spike002@example.invalid"], root)
    run_checked(["git", "add", "seed.txt"], root)
    run_checked(["git", "commit", "--quiet", "-m", "probe baseline"], root)
    return root


def process_attempt(argv: list[str], cwd: Path, raw_path: Path) -> dict[str, object]:
    started = time.monotonic()
    timed_out = False
    try:
        completed = subprocess.run(
            argv,
            cwd=cwd,
            env=minimal_environment(),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=180,
            check=False,
        )
        stdout = completed.stdout
        stderr = completed.stderr
        exit_code: int | None = completed.returncode
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        stdout = exc.stdout or b""
        stderr = exc.stderr or b""
        exit_code = None
    elapsed_ms = int((time.monotonic() - started) * 1000)
    raw = stdout + b"\n--STDERR--\n" + stderr
    raw_path.write_bytes(raw)
    return {
        "exit_code": exit_code,
        "timed_out": timed_out,
        "elapsed_ms": elapsed_ms,
        "stdout": stdout,
        "stderr": stderr,
        "raw_sha256": sha256_bytes(raw),
    }


def json_objects(raw: bytes) -> tuple[list[dict[str, object]], bool]:
    objects: list[dict[str, object]] = []
    ok = True
    for line in raw.decode("utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            ok = False
            continue
        if not isinstance(value, dict):
            ok = False
            continue
        objects.append(value)
    return objects, ok and bool(objects)


def recursive_values(value: object, keys: set[str]) -> list[str]:
    found: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            if key in keys and isinstance(child, str) and child.strip():
                found.append(child.strip())
            found.extend(recursive_values(child, keys))
    elif isinstance(value, list):
        for child in value:
            found.extend(recursive_values(child, keys))
    return found


def claude_system_native_type(subtype: object) -> str:
    if not isinstance(subtype, str) or not subtype.strip():
        raise RuntimeError("Claude system event is missing a nonblank subtype")
    return f"system.{subtype.strip()}"


def native_type(provider: str, value: dict[str, object]) -> str:
    for key in TYPE_KEYS:
        item = value.get(key)
        if isinstance(item, str) and item:
            if provider == "claude" and item == "system":
                return claude_system_native_type(value.get("subtype"))
            return item
    return "unknown"


def normalized_type(provider: str, native: str) -> str:
    try:
        return NORMALIZED_EVENT_TYPES[provider][native]
    except KeyError as exc:
        raise RuntimeError(f"unknown native event type for {provider}: {native}") from exc


def normalize_objects(
    provider: str,
    turn: int,
    objects: list[dict[str, object]],
    cwd_digest: str,
    task_contract_sha256: str,
    prompt_sha256: str,
    start_sequence: int,
    exit_code: int | None,
    timed_out: bool,
) -> tuple[list[dict[str, object]], list[str], list[str]]:
    events: list[dict[str, object]] = []
    sessions: list[str] = []
    types: list[str] = []
    for offset, value in enumerate(objects):
        native = native_type(provider, value)
        types.append(native)
        observed = recursive_values(value, SESSION_KEYS)
        sessions.extend(observed)
        events.append(
            {
                "schema": EVENT_SCHEMA,
                "provider": provider,
                "turn": turn,
                "sequence": start_sequence + offset,
                "native_event_type": native,
                "normalized_event_type": normalized_type(provider, native),
                "session_id": observed[0] if observed else None,
                "cwd_digest": cwd_digest,
                "task_contract_sha256": task_contract_sha256,
                "prompt_sha256": prompt_sha256,
                "exit_code": exit_code if offset == len(objects) - 1 else None,
                "timed_out": timed_out if offset == len(objects) - 1 else False,
            }
        )
    return events, sessions, types


def event_mapping_from_events(provider: str, events: list[dict[str, object]]) -> dict[str, str]:
    mapping: dict[str, str] = {}
    for event in events:
        native = event.get("native_event_type")
        normalized = event.get("normalized_event_type")
        if not isinstance(native, str) or not native:
            raise RuntimeError("normalized event is missing a native type")
        if not isinstance(normalized, str) or not normalized:
            raise RuntimeError("normalized event is missing a normalized type")
        if normalized != normalized_type(provider, native):
            raise RuntimeError(f"native event type was normalized incorrectly: {native}")
        previous = mapping.setdefault(native, normalized)
        if previous != normalized:
            raise RuntimeError(f"native event type mapped inconsistently: {native}")
    return dict(sorted(mapping.items()))


def first_session(provider: str, supplied: str | None, observed: list[str]) -> str | None:
    if provider == "codex":
        return observed[0] if observed else None
    if supplied and supplied in observed:
        return supplied
    return observed[0] if observed else None


def has_exact_session_continuity(
    observed_session: str | None,
    resume_session: str | None,
    observed_second: str | None,
) -> bool:
    return bool(
        observed_session
        and resume_session
        and observed_second
        and observed_session == resume_session == observed_second
    )


def empty_results() -> dict[str, object]:
    return {"schema": SCHEMA, "task_contract": task_contract(), "providers": {}, "all_pass": False}


def load_results() -> dict[str, object]:
    if RESULTS_PATH.exists():
        value = json.loads(RESULTS_PATH.read_text(encoding="utf-8"))
        if (
            isinstance(value, dict)
            and value.get("schema") == SCHEMA
            and isinstance(value.get("task_contract"), dict)
            and isinstance(value.get("providers"), dict)
        ):
            return value
    return empty_results()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def render_report(results: dict[str, object]) -> None:
    providers = results.get("providers", {})
    contract = results.get("task_contract")
    assert isinstance(providers, dict) and isinstance(contract, dict)
    lines = [
        "# SPIKE-002 Installed Provider CLI Feasibility Report",
        "",
        "This report records a bounded two-turn disposable-worktree experiment. It does not claim a production adapter or coordinator.",
        "",
        "| Provider | Result | Version | Exact-session resume | Expected file | Reason |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for provider in PROVIDERS:
        row = providers.get(provider)
        if not isinstance(row, dict):
            continue
        reason = str(row.get("reason", "")).replace("|", "/")
        lines.append(
            f"| {provider} | {row['status']} | {row['version']} | "
            f"{row['session_continuity']} | {row['expected_content']} | {reason} |"
        )
    lines.extend(
        [
            "",
            "## Task contract",
            "",
            f"- Schema: {contract['schema']}",
            f"- Version: {contract['version']}",
            f"- Turn-one prompt digest: {contract['turn_one_prompt_sha256']}",
            f"- Turn-two prompt digest: {contract['turn_two_prompt_sha256']}",
            f"- Expected-final digest: {contract['expected_final_sha256']}",
            f"- Task contract digest: {contract['task_contract_sha256']}",
            "",
            "## Evidence",
            "",
        ]
    )
    for provider in PROVIDERS:
        row = providers.get(provider)
        if not isinstance(row, dict):
            continue
        lines.extend(
            [
                f"### {provider}",
                "",
                f"- Executable: {row['executable']}",
                f"- Version: {row['version']}",
                f"- Result: {row['status']} — {row['reason']}",
                f"- Task contract digest: {row['task_contract_sha256']}",
                f"- Turn-one prompt digest: {row['turn_one_prompt_sha256']}",
                f"- Turn-two prompt digest: {row['turn_two_prompt_sha256']}",
                f"- Working-directory token/digest: {row['cwd_token']} / {row['cwd_digest']}",
                f"- Observed/resumed session: {row['observed_session_id']} / {row['resume_session_id']}",
                f"- Turn-two observed session: {row['turn_two_observed_session_id']}",
                f"- Turn exits: {row['turn_one']['exit_code']} / {row['turn_two']['exit_code']}",
                f"- Raw stream digests: {row['turn_one']['raw_sha256']} / {row['turn_two']['raw_sha256']}",
                f"- Normalized artifact: {row['event_artifact']} ({row['event_artifact_sha256']})",
                f"- Final file digest: {row['final_file_sha256']}",
                f"- Permission behavior: {row['permission_behavior']}",
                f"- Native event types: {', '.join(row['structured_event_types']) or 'none'}",
                f"- Exact native-to-normalized mapping: {json.dumps(row['event_mapping'], ensure_ascii=False, sort_keys=True)}",
                f"- Unavailable/unstable fields: {', '.join(row['unavailable_fields']) or 'none'}",
                f"- Turn-one argv: {json.dumps(row['turn_one']['argv'], ensure_ascii=False)}",
                f"- Turn-two argv: {json.dumps(row['turn_two']['argv'], ensure_ascii=False)}",
                "",
            ]
        )
    lines.extend(
        [
            "## Custody and limitations",
            "",
            "Only normalized metadata is committed. Raw stdout and stderr were hashed and deleted from probe scratch storage after normalization.",
            "No prompt, free-form provider output, reasoning, credential, environment dump, or raw scratch path is retained.",
            "Provider-native event mapping is version-sensitive; unavailable fields are listed per provider.",
            "",
        ]
    )
    REPORT_PATH.write_text("\n".join(lines), encoding="utf-8")


def probe(provider: str) -> dict[str, object]:
    executable = executable_for(provider)
    cwd = create_disposable_repository(provider)
    cwd_digest = sha256_text(str(cwd.resolve()))
    cwd_token = f"$PROBE_ROOT/{provider}"
    version_raw = run_checked(version_argv(provider, executable), cwd)
    version = version_raw.decode("utf-8", errors="replace").strip().splitlines()[0]
    help_raw = run_checked(help_argv(provider, executable), cwd)
    supplied_session = None if provider == "codex" else str(uuid.uuid4())
    contract, (turn_one_prompt, turn_one_prompt_sha256), (turn_two_prompt, turn_two_prompt_sha256) = verified_task_inputs()
    turn_one_argv = provider_argv(provider, executable, turn_one_prompt, supplied_session, 1)
    raw_one = SCRATCH_ROOT / f"{provider}-turn-one.raw"
    first = process_attempt(turn_one_argv, cwd, raw_one)
    objects_one, structured_one = json_objects(first["stdout"])
    events_one, sessions_one, types_one = normalize_objects(
        provider,
        1,
        objects_one,
        cwd_digest,
        str(contract["task_contract_sha256"]),
        turn_one_prompt_sha256,
        1,
        first["exit_code"],
        bool(first["timed_out"]),
    )
    observed_session = first_session(provider, supplied_session, sessions_one)
    resume_session = observed_session or supplied_session
    raw_two = SCRATCH_ROOT / f"{provider}-turn-two.raw"
    if resume_session:
        second_contract, _verified_one, (turn_two_prompt, turn_two_prompt_sha256) = verified_task_inputs()
        if second_contract != contract:
            raise RuntimeError("task contract changed between provider turns")
        turn_two_argv = provider_argv(provider, executable, turn_two_prompt, resume_session, 2)
        second = process_attempt(turn_two_argv, cwd, raw_two)
        objects_two, structured_two = json_objects(second["stdout"])
    else:
        turn_two_argv = []
        second = {
            "exit_code": None,
            "timed_out": False,
            "elapsed_ms": 0,
            "stdout": b"",
            "stderr": b"",
            "raw_sha256": sha256_bytes(b""),
        }
        objects_two, structured_two = [], False
    events_two, sessions_two, types_two = normalize_objects(
        provider,
        2,
        objects_two,
        cwd_digest,
        str(contract["task_contract_sha256"]),
        turn_two_prompt_sha256,
        len(events_one) + 1,
        second["exit_code"],
        bool(second["timed_out"]),
    )
    all_events = events_one + events_two
    EVENT_DIR.mkdir(parents=True, exist_ok=True)
    event_path = EVENT_DIR / f"{provider}.jsonl"
    event_bytes = b"".join(
        (json.dumps(event, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
        for event in all_events
    )
    event_path.write_bytes(event_bytes)
    final_path = cwd / "probe-result.txt"
    final_bytes = final_path.read_bytes() if final_path.exists() else b""
    git_status = run_checked(["git", "status", "--porcelain=v1"], cwd).decode("utf-8", errors="replace")
    observed_second = first_session(provider, supplied_session, sessions_two)
    session_continuity = has_exact_session_continuity(observed_session, resume_session, observed_second)
    expected_content = final_bytes == EXPECTED_BYTES
    failures: list[str] = []
    if first["timed_out"] or second["timed_out"]:
        failures.append("provider turn timed out")
    if first["exit_code"] != 0 or second["exit_code"] != 0:
        failures.append("provider turn returned nonzero or missing exit")
    if not structured_one or not structured_two:
        failures.append("structured output was absent or not fully parseable")
    if not observed_session:
        failures.append("no nonblank session identifier was observed")
    if not session_continuity:
        failures.append("exact-session continuity was not demonstrated")
    if not expected_content:
        failures.append("final file did not match expected bytes")
    status = "PASS" if not failures else "FAIL"
    reason = "all required capabilities observed" if status == "PASS" else "; ".join(failures)
    for raw_path in (raw_one, raw_two):
        if raw_path.exists():
            raw_path.unlink()
    if provider == "copilot":
        for prompt_path in (SCRATCH_ROOT / "turn-one.input", SCRATCH_ROOT / "turn-two.input"):
            if prompt_path.exists():
                prompt_path.unlink()
    return {
        "provider": provider,
        "status": status,
        "task_contract_sha256": contract["task_contract_sha256"],
        "turn_one_prompt_sha256": turn_one_prompt_sha256,
        "turn_two_prompt_sha256": turn_two_prompt_sha256,
        "executable": executable,
        "version": version,
        "version_sha256": sha256_bytes(version_raw),
        "help_sha256": sha256_bytes(help_raw),
        "cwd_token": cwd_token,
        "cwd_digest": cwd_digest,
        "git_status": git_status.replace(str(cwd), cwd_token),
        "turn_one": {
            "argv": sanitize_argv(turn_one_argv, turn_one_prompt),
            "task_contract_sha256": contract["task_contract_sha256"],
            "prompt_sha256": turn_one_prompt_sha256,
            "exit_code": first["exit_code"],
            "timed_out": first["timed_out"],
            "elapsed_ms": first["elapsed_ms"],
            "raw_sha256": first["raw_sha256"],
        },
        "turn_two": {
            "argv": sanitize_argv(turn_two_argv, turn_two_prompt),
            "task_contract_sha256": contract["task_contract_sha256"],
            "prompt_sha256": turn_two_prompt_sha256,
            "exit_code": second["exit_code"],
            "timed_out": second["timed_out"],
            "elapsed_ms": second["elapsed_ms"],
            "raw_sha256": second["raw_sha256"],
        },
        "observed_session_id": observed_session,
        "resume_session_id": resume_session,
        "turn_two_observed_session_id": observed_second,
        "session_continuity": session_continuity,
        "event_artifact": f"docs/spikes/SPIKE-002/events/{provider}.jsonl",
        "event_artifact_sha256": sha256_bytes(event_bytes),
        "final_file_sha256": sha256_bytes(final_bytes),
        "expected_content": expected_content,
        "structured_output": structured_one and structured_two,
        "structured_event_types": sorted(set(types_one + types_two)),
        "event_mapping": event_mapping_from_events(provider, all_events),
        "unavailable_fields": list(EXPECTED_PASS_UNAVAILABLE_FIELDS),
        "permission_behavior": PERMISSION_BEHAVIORS[provider],
        "reason": reason,
    }


def discover_claude_system_types() -> list[str]:
    """Observe only Claude's top-level system discriminators in scratch storage."""
    provider = "claude"
    executable = executable_for(provider)
    cwd = create_disposable_repository(provider)
    run_checked(version_argv(provider, executable), cwd)
    run_checked(help_argv(provider, executable), cwd)
    supplied_session = str(uuid.uuid4())
    contract, (turn_one_prompt, _turn_one_prompt_sha256), (turn_two_prompt, _turn_two_prompt_sha256) = verified_task_inputs()
    raw_one = SCRATCH_ROOT / f"{provider}-turn-one.raw"
    raw_two = SCRATCH_ROOT / f"{provider}-turn-two.raw"
    try:
        first = process_attempt(provider_argv(provider, executable, turn_one_prompt, supplied_session, 1), cwd, raw_one)
        objects_one, structured_one = json_objects(first["stdout"])
        observed_session = first_session(provider, supplied_session, recursive_values(objects_one, SESSION_KEYS))
        resume_session = observed_session or supplied_session
        second_contract, _verified_one, (turn_two_prompt, _verified_two) = verified_task_inputs()
        if second_contract != contract:
            raise RuntimeError("task contract changed between Claude discovery turns")
        second = process_attempt(provider_argv(provider, executable, turn_two_prompt, resume_session, 2), cwd, raw_two)
        objects_two, structured_two = json_objects(second["stdout"])
        observed_second = first_session(provider, supplied_session, recursive_values(objects_two, SESSION_KEYS))
        if first["exit_code"] != 0 or second["exit_code"] != 0 or first["timed_out"] or second["timed_out"]:
            raise RuntimeError("Claude structural discovery did not complete both turns")
        if not structured_one or not structured_two:
            raise RuntimeError("Claude structural discovery lacked parseable structured output")
        if not has_exact_session_continuity(observed_session, resume_session, observed_second):
            raise RuntimeError("Claude structural discovery lacked exact session continuity")
        composites = {
            native_type(provider, value)
            for value in objects_one + objects_two
            if value.get("type") == "system"
        }
        if not composites:
            raise RuntimeError("Claude structural discovery observed no top-level system events")
        return sorted(composites)
    finally:
        for raw_path in (raw_one, raw_two):
            if raw_path.exists():
                raw_path.unlink()
        for prompt_path in (SCRATCH_ROOT / "turn-one.input", SCRATCH_ROOT / "turn-two.input"):
            if prompt_path.exists():
                prompt_path.unlink()


def load_normalized_events(path: Path, provider: str) -> list[dict[str, object]]:
    events: list[dict[str, object]] = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not line.strip():
            raise RuntimeError(f"blank normalized event line {number} for {provider}")
        value = json.loads(line)
        if not isinstance(value, dict) or value.get("provider") != provider:
            raise RuntimeError(f"invalid normalized event line {number} for {provider}")
        events.append(value)
    return events


def refresh_derived_artifacts() -> None:
    """Rebuild only derived metadata after a normalization-rule correction."""
    results = load_results()
    contract = require_results_task_contract(results)
    providers = results.get("providers")
    if not isinstance(providers, dict) or set(providers) != set(PROVIDERS):
        raise RuntimeError("cannot refresh an incomplete provider matrix")
    for provider in PROVIDERS:
        row = providers.get(provider)
        if not isinstance(row, dict):
            raise RuntimeError(f"missing provider row: {provider}")
        require_provider_task_binding(row, contract)
        expected_artifact = f"docs/spikes/SPIKE-002/events/{provider}.jsonl"
        if row.get("event_artifact") != expected_artifact:
            raise RuntimeError(f"unexpected event artifact for {provider}")
        event_path = EVENT_DIR / f"{provider}.jsonl"
        events = load_normalized_events(event_path, provider)
        for event in events:
            turn = event.get("turn")
            if turn not in (1, 2):
                raise RuntimeError(f"normalized event has an invalid turn for {provider}")
            if event.get("task_contract_sha256") != contract["task_contract_sha256"]:
                raise RuntimeError(f"normalized event task contract does not match results for {provider}")
            if event.get("prompt_sha256") != contract[prompt_digest_field(turn)]:
                raise RuntimeError(f"normalized event prompt digest does not match results for {provider}")
            native = event.get("native_event_type")
            if not isinstance(native, str) or not native:
                raise RuntimeError(f"normalized event is missing a native type for {provider}")
            event["normalized_event_type"] = normalized_type(provider, native)
        event_bytes = b"".join(
            (json.dumps(event, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
            for event in events
        )
        event_path.write_bytes(event_bytes)
        mapping = event_mapping_from_events(provider, events)
        row["event_artifact_sha256"] = sha256_bytes(event_bytes)
        row["structured_event_types"] = sorted(mapping)
        row["event_mapping"] = mapping
        row["permission_behavior"] = PERMISSION_BEHAVIORS[provider]
    write_json(RESULTS_PATH, results)
    render_report(results)


def run_self_check() -> None:
    def expect_runtime_failure(action, message: str) -> None:
        try:
            action()
        except RuntimeError:
            return
        raise RuntimeError(message)

    session = "session-evidence"
    if has_exact_session_continuity(session, session, None):
        raise RuntimeError("missing turn-two session incorrectly counts as continuous")
    if has_exact_session_continuity(session, session, "other-session"):
        raise RuntimeError("different turn-two session incorrectly counts as continuous")
    if not has_exact_session_continuity(session, session, session):
        raise RuntimeError("matching session evidence incorrectly fails continuity")
    if native_type("claude", {"type": "system", "subtype": "init"}) != "system.init":
        raise RuntimeError("Claude init subtype was not preserved")
    if normalized_type("claude", "system.init") != "session.started":
        raise RuntimeError("Claude init subtype was not the sole session-start mapping")
    if normalized_type("claude", "system.thinking_tokens") != "provider.event":
        raise RuntimeError("Claude non-init system subtype was not provider-neutral")
    if normalized_type("copilot", "session.info") != "provider.event":
        raise RuntimeError("Copilot session.info was not provider-neutral")
    if normalized_type("copilot", "session.info") == "session.started":
        raise RuntimeError("Copilot session.info incorrectly maps to session.started")
    for action, message in (
        (lambda: native_type("claude", {"type": "system"}), "missing Claude system subtype did not fail closed"),
        (lambda: normalized_type("claude", "system.unknown"), "unknown Claude system subtype did not fail closed"),
        (lambda: normalized_type("copilot", "session.unknown"), "unknown Copilot native type did not fail closed"),
        (lambda: normalized_type("codex", "unknown.native.event"), "unknown native event type did not fail closed"),
    ):
        expect_runtime_failure(action, message)

    with tempfile.TemporaryDirectory(prefix="spike002-taskcheck-") as directory:
        root = Path(directory)
        prompt_one = root / "turn-one.input"
        expected_prompt = sha256_bytes(b"self-check canonical prompt")
        prompt_one.write_bytes(b"self-check canonical prompt")
        read_verified_prompt(root, 1, expected_prompt)
        prompt_one.write_bytes(b"self-check changed prompt")
        expect_runtime_failure(
            lambda: read_verified_prompt(root, 1, expected_prompt),
            "changed prompt bytes did not refuse before provider execution",
        )
        expect_runtime_failure(
            lambda: read_verified_prompt(root, 2, sha256_bytes(b"missing prompt")),
            "missing prompt did not refuse before provider execution",
        )

    contract = task_contract()

    def task_bound_row() -> dict[str, object]:
        return {
            "task_contract_sha256": contract["task_contract_sha256"],
            "turn_one_prompt_sha256": contract["turn_one_prompt_sha256"],
            "turn_two_prompt_sha256": contract["turn_two_prompt_sha256"],
            "turn_one": {
                "task_contract_sha256": contract["task_contract_sha256"],
                "prompt_sha256": contract["turn_one_prompt_sha256"],
            },
            "turn_two": {
                "task_contract_sha256": contract["task_contract_sha256"],
                "prompt_sha256": contract["turn_two_prompt_sha256"],
            },
        }

    matrix = empty_results()
    merge_provider_row(matrix, "codex", task_bound_row())
    foreign = task_bound_row()
    foreign["task_contract_sha256"] = sha256_bytes(b"foreign task contract")
    expect_runtime_failure(
        lambda: merge_provider_row(matrix, "claude", foreign),
        "provider rows with different task contracts merged",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--provider", choices=PROVIDERS)
    parser.add_argument("--refresh-derived", action="store_true")
    parser.add_argument("--self-check", action="store_true")
    parser.add_argument("--discover-claude-system-types", action="store_true")
    args = parser.parse_args()
    if args.discover_claude_system_types:
        if args.provider or args.refresh_derived or args.self_check:
            parser.error("--discover-claude-system-types cannot be combined with another action")
        print(json.dumps({"provider": "claude", "system_types": discover_claude_system_types()}, sort_keys=True))
        return 0
    if args.self_check:
        if args.provider or args.refresh_derived:
            parser.error("--self-check cannot be combined with another action")
        run_self_check()
        return 0
    if args.refresh_derived:
        if args.provider:
            parser.error("--refresh-derived cannot be combined with --provider")
        refresh_derived_artifacts()
        return 0
    if not args.provider:
        parser.error("--provider is required unless --refresh-derived is used")
    # Codex is the required first provider. Starting it always discards any
    # prior or partially trusted matrix before the sequential probe begins.
    results = empty_results() if args.provider == PROVIDERS[0] else load_results()
    contract = require_results_task_contract(results)
    providers = results.get("providers")
    assert isinstance(providers, dict)
    try:
        row = probe(args.provider)
    except Exception as exc:  # fail result, never hide a provider/setup failure
        EVENT_DIR.mkdir(parents=True, exist_ok=True)
        empty_events = EVENT_DIR / f"{args.provider}.jsonl"
        empty_events.write_bytes(b"")
        executable = shutil.which("claude" if args.provider == "claude" else args.provider) or "unavailable"
        cwd_token = f"$PROBE_ROOT/{args.provider}"
        row = {
            "provider": args.provider,
            "status": "FAIL",
            "task_contract_sha256": contract["task_contract_sha256"],
            "turn_one_prompt_sha256": contract["turn_one_prompt_sha256"],
            "turn_two_prompt_sha256": contract["turn_two_prompt_sha256"],
            "executable": executable,
            "version": "unavailable",
            "version_sha256": sha256_bytes(b""),
            "help_sha256": sha256_bytes(b""),
            "cwd_token": cwd_token,
            "cwd_digest": sha256_text(cwd_token),
            "git_status": "unavailable",
            "turn_one": {
                "argv": [executable, "<redacted-prompt>"],
                "task_contract_sha256": contract["task_contract_sha256"],
                "prompt_sha256": contract["turn_one_prompt_sha256"],
                "exit_code": None,
                "timed_out": False,
                "elapsed_ms": 0,
                "raw_sha256": sha256_bytes(b""),
            },
            "turn_two": {
                "argv": [executable, "<redacted-prompt>"],
                "task_contract_sha256": contract["task_contract_sha256"],
                "prompt_sha256": contract["turn_two_prompt_sha256"],
                "exit_code": None,
                "timed_out": False,
                "elapsed_ms": 0,
                "raw_sha256": sha256_bytes(b""),
            },
            "observed_session_id": None,
            "resume_session_id": None,
            "turn_two_observed_session_id": None,
            "session_continuity": False,
            "event_artifact": f"docs/spikes/SPIKE-002/events/{args.provider}.jsonl",
            "event_artifact_sha256": sha256_bytes(b""),
            "final_file_sha256": sha256_bytes(b""),
            "expected_content": False,
            "structured_output": False,
            "structured_event_types": [],
            "event_mapping": {"unavailable": "provider.event"},
            "unavailable_fields": ["all provider fields unavailable because the probe harness failed before a complete turn"],
            "permission_behavior": "not observed",
            "reason": f"probe harness failure: {type(exc).__name__}: {exc}",
        }
    merge_provider_row(results, args.provider, row)
    results["all_pass"] = set(providers) == set(PROVIDERS) and all(
        isinstance(providers[name], dict) and providers[name].get("status") == "PASS"
        for name in PROVIDERS
    )
    write_json(RESULTS_PATH, results)
    render_report(results)
    row = providers[args.provider]
    print(json.dumps({"provider": args.provider, "status": row.get("status"), "reason": row.get("reason")}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
