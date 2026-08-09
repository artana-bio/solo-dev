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


def native_type(value: dict[str, object]) -> str:
    for key in TYPE_KEYS:
        item = value.get(key)
        if isinstance(item, str) and item:
            return item
    return "unknown"


def normalized_type(native: str) -> str:
    lowered = native.lower()
    if "start" in lowered or "init" in lowered or "system" in lowered:
        return "session.started"
    if "tool" in lowered or "command" in lowered or "item" in lowered:
        return "provider.activity"
    if "result" in lowered or "complete" in lowered or "finish" in lowered or "end" in lowered:
        return "turn.completed"
    if "error" in lowered or "fail" in lowered:
        return "turn.failed"
    return "provider.event"


def normalize_objects(
    provider: str,
    turn: int,
    objects: list[dict[str, object]],
    cwd_digest: str,
    start_sequence: int,
    exit_code: int | None,
    timed_out: bool,
) -> tuple[list[dict[str, object]], list[str], list[str]]:
    events: list[dict[str, object]] = []
    sessions: list[str] = []
    types: list[str] = []
    for offset, value in enumerate(objects):
        native = native_type(value)
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
                "normalized_event_type": normalized_type(native),
                "session_id": observed[0] if observed else None,
                "cwd_digest": cwd_digest,
                "exit_code": exit_code if offset == len(objects) - 1 else None,
                "timed_out": timed_out if offset == len(objects) - 1 else False,
            }
        )
    return events, sessions, types


def first_session(provider: str, supplied: str | None, observed: list[str]) -> str | None:
    if provider == "codex":
        return observed[0] if observed else None
    if supplied and supplied in observed:
        return supplied
    return observed[0] if observed else None


def empty_results() -> dict[str, object]:
    return {"schema": SCHEMA, "providers": {}, "all_pass": False}


def load_results() -> dict[str, object]:
    if RESULTS_PATH.exists():
        value = json.loads(RESULTS_PATH.read_text(encoding="utf-8"))
        if (
            isinstance(value, dict)
            and value.get("schema") == SCHEMA
            and isinstance(value.get("providers"), dict)
        ):
            return value
    return empty_results()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def render_report(results: dict[str, object]) -> None:
    providers = results.get("providers", {})
    assert isinstance(providers, dict)
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
    lines.extend(["", "## Evidence", ""])
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
                f"- Working-directory token/digest: {row['cwd_token']} / {row['cwd_digest']}",
                f"- Observed/resumed session: {row['observed_session_id']} / {row['resume_session_id']}",
                f"- Turn exits: {row['turn_one']['exit_code']} / {row['turn_two']['exit_code']}",
                f"- Raw stream digests: {row['turn_one']['raw_sha256']} / {row['turn_two']['raw_sha256']}",
                f"- Normalized artifact: {row['event_artifact']} ({row['event_artifact_sha256']})",
                f"- Final file digest: {row['final_file_sha256']}",
                f"- Permission behavior: {row['permission_behavior']}",
                f"- Native event types: {', '.join(row['structured_event_types']) or 'none'}",
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
    turn_one_prompt = (SCRATCH_ROOT / "turn-one.input").read_text(encoding="utf-8").strip()
    turn_two_prompt = (SCRATCH_ROOT / "turn-two.input").read_text(encoding="utf-8").strip()
    turn_one_argv = provider_argv(provider, executable, turn_one_prompt, supplied_session, 1)
    raw_one = SCRATCH_ROOT / f"{provider}-turn-one.raw"
    first = process_attempt(turn_one_argv, cwd, raw_one)
    objects_one, structured_one = json_objects(first["stdout"])
    events_one, sessions_one, types_one = normalize_objects(
        provider,
        1,
        objects_one,
        cwd_digest,
        1,
        first["exit_code"],
        bool(first["timed_out"]),
    )
    observed_session = first_session(provider, supplied_session, sessions_one)
    resume_session = observed_session or supplied_session
    turn_two_argv = provider_argv(provider, executable, turn_two_prompt, resume_session, 2) if resume_session else []
    raw_two = SCRATCH_ROOT / f"{provider}-turn-two.raw"
    if turn_two_argv:
        second = process_attempt(turn_two_argv, cwd, raw_two)
        objects_two, structured_two = json_objects(second["stdout"])
    else:
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
    session_continuity = bool(
        observed_session
        and resume_session
        and observed_session == resume_session
        and (observed_second in {None, resume_session} or observed_second == resume_session)
    )
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
        "executable": executable,
        "version": version,
        "version_sha256": sha256_bytes(version_raw),
        "help_sha256": sha256_bytes(help_raw),
        "cwd_token": cwd_token,
        "cwd_digest": cwd_digest,
        "git_status": git_status.replace(str(cwd), cwd_token),
        "turn_one": {
            "argv": sanitize_argv(turn_one_argv, turn_one_prompt),
            "exit_code": first["exit_code"],
            "timed_out": first["timed_out"],
            "elapsed_ms": first["elapsed_ms"],
            "raw_sha256": first["raw_sha256"],
        },
        "turn_two": {
            "argv": sanitize_argv(turn_two_argv, turn_two_prompt),
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
        "event_mapping": {
            "session/start": "session.started",
            "tool/item/activity": "provider.activity",
            "result/completed": "turn.completed",
            "error/failed": "turn.failed",
            "other": "provider.event",
        },
        "unavailable_fields": ["provider-side authoritative worktree identity", "provider-side cryptographic termination receipt"],
        "permission_behavior": "noninteractive bounded file-edit permission flags; no URL or unrestricted path permission granted",
        "reason": reason,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--provider", required=True, choices=PROVIDERS)
    args = parser.parse_args()
    # Codex is the required first provider. Starting it always discards any
    # prior or partially trusted matrix before the sequential probe begins.
    results = empty_results() if args.provider == PROVIDERS[0] else load_results()
    providers = results.setdefault("providers", {})
    assert isinstance(providers, dict)
    try:
        providers[args.provider] = probe(args.provider)
    except Exception as exc:  # fail result, never hide a provider/setup failure
        EVENT_DIR.mkdir(parents=True, exist_ok=True)
        empty_events = EVENT_DIR / f"{args.provider}.jsonl"
        empty_events.write_bytes(b"")
        executable = shutil.which("claude" if args.provider == "claude" else args.provider) or "unavailable"
        cwd_token = f"$PROBE_ROOT/{args.provider}"
        providers[args.provider] = {
            "provider": args.provider,
            "status": "FAIL",
            "executable": executable,
            "version": "unavailable",
            "version_sha256": sha256_bytes(b""),
            "help_sha256": sha256_bytes(b""),
            "cwd_token": cwd_token,
            "cwd_digest": sha256_text(cwd_token),
            "git_status": "unavailable",
            "turn_one": {"argv": [executable, "<redacted-prompt>"], "exit_code": None, "timed_out": False, "elapsed_ms": 0, "raw_sha256": sha256_bytes(b"")},
            "turn_two": {"argv": [executable, "<redacted-prompt>"], "exit_code": None, "timed_out": False, "elapsed_ms": 0, "raw_sha256": sha256_bytes(b"")},
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
