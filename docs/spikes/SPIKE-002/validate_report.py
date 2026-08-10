#!/usr/bin/env python3
"""Fail-closed validator for the SPIKE-002 provider feasibility evidence."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import re
import sys
import tempfile


SCHEMA = "harness.provider-feasibility-report/v1"
EVENT_SCHEMA = "harness.provider-feasibility-event/v1"
PROVIDERS = ("codex", "claude", "copilot")
SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[2]
REPORT_PATH = SCRIPT_DIR.parent / "SPIKE-002-REPORT.md"
RESULTS_PATH = SCRIPT_DIR / "results.json"
EVENT_DIR = SCRIPT_DIR / "events"
PLAN_PATH = REPO_ROOT / "docs" / "IMPLEMENTATION_PLAN.md"
EXPECTED_FINAL_FILE_SHA256 = "sha256:40e9f7eb05f53663dada2f6e9dc91c49c1113e42ed88cdffa81278b3aafa6f9d"
EXPECTED_ARTIFACT_RELATIVE_PATHS = frozenset(
    {
        "run_probe.py",
        "validate_report.py",
        "results.json",
        "events/codex.jsonl",
        "events/claude.jsonl",
        "events/copilot.jsonl",
    }
)
EVENT_KEYS = {
    "schema",
    "provider",
    "turn",
    "sequence",
    "native_event_type",
    "normalized_event_type",
    "session_id",
    "cwd_digest",
    "exit_code",
    "timed_out",
}
ROW_KEYS = {
    "provider",
    "status",
    "executable",
    "version",
    "version_sha256",
    "help_sha256",
    "cwd_token",
    "cwd_digest",
    "git_status",
    "turn_one",
    "turn_two",
    "observed_session_id",
    "resume_session_id",
    "turn_two_observed_session_id",
    "session_continuity",
    "event_artifact",
    "event_artifact_sha256",
    "final_file_sha256",
    "expected_content",
    "structured_output",
    "structured_event_types",
    "event_mapping",
    "unavailable_fields",
    "permission_behavior",
    "reason",
}
TURN_KEYS = {"argv", "exit_code", "timed_out", "elapsed_ms", "raw_sha256"}
CANONICAL_SHA256 = re.compile(r"sha256:[0-9a-f]{64}\Z")
ROW_CUSTODY_DIGEST_FIELDS = (
    "version_sha256",
    "help_sha256",
    "cwd_digest",
    "event_artifact_sha256",
    "final_file_sha256",
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
FORBIDDEN_EXACT_FLAGS = {
    "--allow-all-paths",
    "--allow-all-urls",
    "--allow-all",
    "--yolo",
    "--dangerously-bypass-approvals-and-sandbox",
    "--dangerously-skip-permissions",
    "--no-sandbox",
}
FORBIDDEN_VALUE_FLAGS = {
    "--allow-all-paths",
    "--allow-all-urls",
    "--allow-all",
    "--yolo",
    "--dangerously-bypass-approvals-and-sandbox",
    "--dangerously-skip-permissions",
    "--sandbox",
    "--permission-mode",
}


class ValidationError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValidationError(message)


def sha256_bytes(value: bytes) -> str:
    return "sha256:" + hashlib.sha256(value).hexdigest()


def require_canonical_sha256(value: object, label: str) -> str:
    require(
        isinstance(value, str) and CANONICAL_SHA256.fullmatch(value) is not None,
        f"{label} must be a canonical sha256 digest",
    )
    return value


def normalized_event_type(provider: str, native: str) -> str:
    try:
        return NORMALIZED_EVENT_TYPES[provider][native]
    except KeyError as exc:
        raise ValidationError(f"unknown native event type for {provider}: {native}") from exc


def load_json(path: Path) -> dict[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValidationError(f"cannot parse {path.name}: {exc}") from exc
    require(isinstance(value, dict), f"{path.name} must contain one object")
    return value


def parse_events_bytes(raw: bytes, provider: str) -> list[dict[str, object]]:
    require(raw.endswith(b"\n") or raw == b"", f"{provider} JSONL is truncated")
    events: list[dict[str, object]] = []
    for number, line in enumerate(raw.splitlines(), start=1):
        require(bool(line.strip()), f"{provider} JSONL line {number} is blank")
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValidationError(f"{provider} JSONL line {number} is malformed") from exc
        require(isinstance(value, dict), f"{provider} JSONL line {number} is not an object")
        require(set(value) == EVENT_KEYS, f"{provider} JSONL line {number} has unknown/missing fields")
        require(value["schema"] == EVENT_SCHEMA, f"{provider} JSONL line {number} schema mismatch")
        require(value["provider"] == provider, f"{provider} JSONL line {number} provider mismatch")
        require(value["turn"] in (1, 2), f"{provider} JSONL line {number} invalid turn")
        require(value["sequence"] == number, f"{provider} JSONL sequence is not monotonic")
        native = value["native_event_type"]
        normalized = value["normalized_event_type"]
        require(isinstance(native, str) and native, f"{provider} JSONL line {number} native type missing")
        require(isinstance(normalized, str) and normalized, f"{provider} JSONL line {number} normalized type missing")
        require(
            normalized == normalized_event_type(provider, native),
            f"{provider} JSONL line {number} normalization mismatch",
        )
        require_canonical_sha256(value["cwd_digest"], f"{provider} JSONL line {number} cwd digest")
        require(
            value["session_id"] is None or isinstance(value["session_id"], str),
            f"{provider} JSONL line {number} session identifier invalid",
        )
        require(
            value["exit_code"] is None or isinstance(value["exit_code"], int),
            f"{provider} JSONL line {number} exit code invalid",
        )
        require(isinstance(value["timed_out"], bool), f"{provider} JSONL line {number} timeout invalid")
        events.append(value)
    return events


def derive_event_mapping(provider: str, events: list[dict[str, object]]) -> dict[str, str]:
    mapping: dict[str, str] = {}
    for event in events:
        native = event["native_event_type"]
        normalized = event["normalized_event_type"]
        require(isinstance(native, str) and native, "event native type missing")
        require(isinstance(normalized, str) and normalized, "event normalized type missing")
        require(normalized == normalized_event_type(provider, native), "event normalization mismatch")
        previous = mapping.setdefault(native, normalized)
        require(previous == normalized, f"native event type maps inconsistently: {native}")
    return dict(sorted(mapping.items()))


def assert_no_forbidden_flags(argv: list[str], provider: str, label: str) -> None:
    for index, argument in enumerate(argv):
        lowered = argument.lower()
        require(argument not in FORBIDDEN_EXACT_FLAGS, f"{provider} {label} used forbidden flag {argument}")
        for flag in FORBIDDEN_VALUE_FLAGS - {"--sandbox", "--permission-mode"}:
            require(
                not lowered.startswith(flag + "="),
                f"{provider} {label} used forbidden flag {argument}",
            )
        if argument == "--allow-all-tools":
            require(provider == "copilot", f"{provider} {label} used Copilot-only auto-approval")
        if argument == "--sandbox" and index + 1 < len(argv):
            require(
                argv[index + 1].lower() not in {"danger-full-access", "dangerfullaccess"},
                f"{provider} {label} used unrestricted sandbox mode",
            )
        require(
            not lowered.startswith("--sandbox=danger-full-access"),
            f"{provider} {label} used unrestricted sandbox mode",
        )
        if argument == "--permission-mode" and index + 1 < len(argv):
            require(
                argv[index + 1].lower() not in {"bypasspermissions", "bypass-permissions", "bypass"},
                f"{provider} {label} used a permission bypass mode",
            )
        require(
            not lowered.startswith("--permission-mode=bypass"),
            f"{provider} {label} used a permission bypass mode",
        )


def validate_turn(turn: object, provider: str, label: str) -> dict[str, object]:
    require(isinstance(turn, dict), f"{provider} {label} must be an object")
    require(set(turn) == TURN_KEYS, f"{provider} {label} fields mismatch")
    argv = turn["argv"]
    require(isinstance(argv, list) and argv, f"{provider} {label} argv missing")
    require(all(isinstance(value, str) and value for value in argv), f"{provider} {label} argv is invalid")
    require(argv.count("<redacted-prompt>") == 1, f"{provider} {label} prompt was not redacted")
    require(argv[-1] == "<redacted-prompt>", f"{provider} {label} prompt is not terminal")
    require(isinstance(turn["elapsed_ms"], int) and turn["elapsed_ms"] >= 0, f"{provider} {label} elapsed invalid")
    require(isinstance(turn["timed_out"], bool), f"{provider} {label} timeout invalid")
    require_canonical_sha256(turn["raw_sha256"], f"{provider} {label} raw digest")
    assert_no_forbidden_flags(argv, provider, label)
    return turn


def validate_custody_digests(provider: str, row: dict[str, object]) -> None:
    for field in ROW_CUSTODY_DIGEST_FIELDS:
        require_canonical_sha256(row.get(field), f"{provider} {field}")
    for turn_label in ("turn_one", "turn_two"):
        turn = row.get(turn_label)
        require(isinstance(turn, dict), f"{provider} {turn_label} must be an object")
        require_canonical_sha256(turn.get("raw_sha256"), f"{provider} {turn_label} raw digest")


def expected_pass_argvs(provider: str, executable: str, session: str) -> tuple[list[str], list[str]]:
    prompt = "<redacted-prompt>"
    if provider == "codex":
        return (
            [executable, "exec", "--json", "--sandbox", "workspace-write", "--skip-git-repo-check", prompt],
            [executable, "exec", "resume", session, "--json", prompt],
        )
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
        return (common + ["--session-id", session, prompt], common + ["--resume", session, prompt])
    if provider == "copilot":
        common = [executable, "--output-format", "json", "--allow-all-tools"]
        return (
            common + ["--session-id", session, "--prompt", prompt],
            common + [f"--resume={session}", "--prompt", prompt],
        )
    raise ValidationError(f"unknown provider: {provider}")


def validate_event_bindings(provider: str, row: dict[str, object], events: list[dict[str, object]]) -> None:
    row_cwd_digest = require_canonical_sha256(row.get("cwd_digest"), f"{provider} cwd_digest")
    for number, event in enumerate(events, start=1):
        event_cwd_digest = require_canonical_sha256(
            event.get("cwd_digest"), f"{provider} JSONL line {number} cwd digest"
        )
        require(event_cwd_digest == row_cwd_digest, f"{provider} event cwd binding mismatch")
    mapping = derive_event_mapping(provider, events)
    require(row["event_mapping"] == mapping, f"{provider} event mapping does not match normalized JSONL")
    require(
        row["structured_event_types"] == sorted(mapping),
        f"{provider} event types do not match normalized JSONL",
    )


def require_nonblank_session(value: object, provider: str, field: str) -> str:
    require(isinstance(value, str) and value.strip(), f"{provider} PASS lacks {field}")
    return value


def turn_session_ids(provider: str, events: list[dict[str, object]], turn: int) -> set[str]:
    session_ids: set[str] = set()
    for event in events:
        if event["turn"] != turn or event["session_id"] is None:
            continue
        session_id = event["session_id"]
        require(
            isinstance(session_id, str) and session_id.strip(),
            f"{provider} turn-{turn} JSONL contains a blank session identifier",
        )
        session_ids.add(session_id)
    return session_ids


def validate_native_session_evidence(
    provider: str,
    observed: str,
    resumed: str,
    events: list[dict[str, object]],
) -> None:
    turn_one = turn_session_ids(provider, events, 1)
    turn_two = turn_session_ids(provider, events, 2)
    require(observed in turn_one, f"{provider} turn-one JSONL lacks the observed session")
    require(turn_one == {observed}, f"{provider} turn-one JSONL contains a different session")
    require(resumed in turn_two, f"{provider} turn-two JSONL lacks the resumed session")
    require(turn_two == {resumed}, f"{provider} turn-two JSONL contains a different session")


def validate_pass_requirements(provider: str, row: dict[str, object], events: list[dict[str, object]]) -> None:
    first = validate_turn(row["turn_one"], provider, "turn_one")
    second = validate_turn(row["turn_two"], provider, "turn_two")
    validate_custody_digests(provider, row)
    observed = require_nonblank_session(row["observed_session_id"], provider, "observed session")
    resumed = require_nonblank_session(row["resume_session_id"], provider, "resumed session")
    second_observed = require_nonblank_session(
        row["turn_two_observed_session_id"], provider, "turn-two observed session"
    )
    require(observed == resumed == second_observed, f"{provider} PASS session continuity is not exact")
    validate_native_session_evidence(provider, observed, resumed, events)
    executable = row["executable"]
    require(isinstance(executable, str) and executable, f"{provider} executable missing")
    expected_first, expected_second = expected_pass_argvs(provider, executable, observed)
    require(first["argv"] == expected_first, f"{provider} PASS turn-one argv does not match required shape")
    require(second["argv"] == expected_second, f"{provider} PASS turn-two argv does not match required shape")
    require(first["argv"][0] == executable and second["argv"][0] == executable, f"{provider} argv executable mismatch")
    require(first["exit_code"] == 0 and second["exit_code"] == 0, f"{provider} PASS has nonzero exit")
    require(not first["timed_out"] and not second["timed_out"], f"{provider} PASS timed out")
    require(row["structured_output"] is True and bool(events), f"{provider} PASS lacks structured events")
    require(row["session_continuity"] is True, f"{provider} PASS continuity is false")
    require(row["expected_content"] is True, f"{provider} PASS final content mismatch")
    expected_final_digest = require_canonical_sha256(EXPECTED_FINAL_FILE_SHA256, "expected final file digest")
    require(row["final_file_sha256"] == expected_final_digest, f"{provider} PASS final file digest does not match expected bytes")
    require(
        row["permission_behavior"] == PERMISSION_BEHAVIORS[provider],
        f"{provider} permission statement does not match exact argv",
    )
    validate_event_bindings(provider, row, events)


def validate_row(provider: str, row: object) -> tuple[dict[str, object], list[dict[str, object]]]:
    require(isinstance(row, dict), f"{provider} row must be an object")
    require(set(row) == ROW_KEYS, f"{provider} row has unknown/missing fields")
    require(row["provider"] == provider, f"{provider} row identity mismatch")
    require(row["status"] in ("PASS", "FAIL"), f"{provider} result must be PASS or FAIL")
    for field in (
        "executable",
        "version",
        "version_sha256",
        "help_sha256",
        "cwd_token",
        "cwd_digest",
        "event_artifact",
        "event_artifact_sha256",
        "final_file_sha256",
        "permission_behavior",
        "reason",
    ):
        require(isinstance(row[field], str) and row[field].strip(), f"{provider} {field} missing")
    validate_custody_digests(provider, row)
    require(row["cwd_token"] == f"$PROBE_ROOT/{provider}", f"{provider} cwd token mismatch")
    first = validate_turn(row["turn_one"], provider, "turn_one")
    second = validate_turn(row["turn_two"], provider, "turn_two")
    event_path = REPO_ROOT / str(row["event_artifact"])
    require(event_path == EVENT_DIR / f"{provider}.jsonl", f"{provider} event path escapes expected location")
    raw = event_path.read_bytes()
    require(sha256_bytes(raw) == row["event_artifact_sha256"], f"{provider} event digest mismatch")
    events = parse_events_bytes(raw, provider)
    require(isinstance(row["structured_event_types"], list), f"{provider} event types missing")
    require(all(isinstance(value, str) and value for value in row["structured_event_types"]), f"{provider} event types invalid")
    require(isinstance(row["event_mapping"], dict), f"{provider} event mapping missing")
    require(isinstance(row["unavailable_fields"], list), f"{provider} unavailable fields missing")
    if events:
        validate_event_bindings(provider, row, events)
    if row["status"] == "PASS":
        validate_pass_requirements(provider, row, events)
    else:
        require(str(row["reason"]).strip() not in ("FAIL", "unknown"), f"{provider} FAIL reason is not specific")
        require(not row["expected_content"] or row["final_file_sha256"] == EXPECTED_FINAL_FILE_SHA256, f"{provider} FAIL content claim is unbound")
        require(first["argv"][0] == row["executable"], f"{provider} FAIL turn-one executable mismatch")
        require(second["argv"][0] == row["executable"], f"{provider} FAIL turn-two executable mismatch")
    return row, events


def validate_candidate_file_set(root: Path = SCRIPT_DIR) -> None:
    actual = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file()
    }
    require(
        actual == EXPECTED_ARTIFACT_RELATIVE_PATHS,
        "unexpected or missing SPIKE-002 artifact files: " + ", ".join(sorted(actual ^ EXPECTED_ARTIFACT_RELATIVE_PATHS)),
    )


def validate_executable_exemption(executable: str) -> None:
    require(executable.startswith("/"), "recorded executable must be an absolute path")
    require(not executable.endswith("/"), "recorded executable must not be a path prefix")
    require(len(Path(executable).name) > 1, "recorded executable name is too short")


def scan_text(
    path: Path,
    text: str,
    executable_values: list[str],
    payload_artifact: bool,
) -> None:
    scrubbed = text
    for executable in executable_values:
        validate_executable_exemption(executable)
        require(
            executable + "/" not in text,
            f"recorded executable is used as a raw path prefix in {path.name}",
        )
        scrubbed = scrubbed.replace(executable, "<allowed-executable>")
    secret_sentinel = "SPIKE_" + "SECRET_" + "SENTINEL"
    reasoning_sentinel = "SPIKE_" + "REASONING_" + "SENTINEL"
    home_prefix = "/" + "Users" + "/"
    raw_scratch_prefixes = (
        "/" + "tmp" + "/",
        "/" + "private" + "/" + "tmp" + "/",
        "/" + "var" + "/" + "folders" + "/",
    )
    token_pattern = re.compile(r"(?:sk-[A-Za-z0-9_-]{16,}|ghp_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,})")
    require(secret_sentinel not in scrubbed, f"forbidden secret sentinel in {path.name}")
    require(reasoning_sentinel not in scrubbed, f"forbidden reasoning sentinel in {path.name}")
    require(home_prefix not in scrubbed, f"unrestricted home path in {path.name}")
    for prefix in raw_scratch_prefixes:
        require(prefix not in scrubbed, f"raw scratch path in {path.name}")
    require(not token_pattern.search(scrubbed), f"token-like value in {path.name}")
    if payload_artifact:
        lowered = scrubbed.lower()
        require('"stdout"' not in lowered and '"stderr"' not in lowered, f"raw stream field in {path.name}")
        require(
            '"reasoning"' not in lowered and '"message"' not in lowered and '"content"' not in lowered,
            f"free-form payload field in {path.name}",
        )


def scan_candidate(results: dict[str, object], report_path: Path) -> None:
    validate_candidate_file_set()
    providers = results.get("providers")
    require(isinstance(providers, dict), "providers must be an object")
    executable_values: list[str] = []
    for provider in PROVIDERS:
        row = providers.get(provider)
        require(isinstance(row, dict) and isinstance(row.get("executable"), str), f"{provider} executable missing")
        executable_values.append(row["executable"])
    paths = [report_path] + [SCRIPT_DIR / relative for relative in sorted(EXPECTED_ARTIFACT_RELATIVE_PATHS)]
    for path in paths:
        require(path.is_file(), f"expected artifact is missing: {path.name}")
        payload_artifact = path == report_path or path.name == "results.json" or path.suffix == ".jsonl"
        scan_text(path, path.read_text(encoding="utf-8"), executable_values, payload_artifact)


def rendered_reason(row: dict[str, object]) -> str:
    return str(row["reason"]).replace("|", "/")


def report_summary_rows(report: str) -> dict[str, list[str]]:
    header = "| Provider | Result | Version | Exact-session resume | Expected file | Reason |"
    separator = "| --- | --- | --- | --- | --- | --- |"
    lines = report.splitlines()
    require(lines.count(header) == 1, "report summary header is missing or duplicated")
    index = lines.index(header)
    require(index + 1 < len(lines) and lines[index + 1] == separator, "report summary separator is missing")
    rows: dict[str, list[str]] = {}
    end = index + 2
    while end < len(lines) and lines[end].startswith("|"):
        line = lines[end]
        require(line.endswith("|"), "report summary row has malformed delimiters")
        cells = [cell.strip() for cell in line.split("|")[1:-1]]
        require(len(cells) == 6, "report summary row has the wrong column count")
        provider = cells[0]
        require(provider in PROVIDERS, f"unexpected provider in report summary: {provider}")
        require(provider not in rows, f"duplicate provider in report summary: {provider}")
        rows[provider] = cells
        end += 1
    require(set(rows) == set(PROVIDERS), "report summary provider matrix is incomplete")
    require(
        not any(line.lstrip().startswith("|") for line in lines[end:]),
        "report has a summary row or table outside the closed summary",
    )
    return rows


def validate_report_summary(provider: str, row: dict[str, object], summary: dict[str, list[str]]) -> None:
    expected = [
        provider,
        str(row["status"]),
        str(row["version"]),
        str(row["session_continuity"]),
        str(row["expected_content"]),
        rendered_reason(row),
    ]
    require(summary.get(provider) == expected, f"report summary does not agree with {provider}")


def provider_report_section(report: str, provider: str) -> str:
    heading = f"### {provider}"
    lines = report.splitlines()
    matches = [index for index, line in enumerate(lines) if line == heading]
    require(len(matches) == 1, f"report has missing or duplicate {provider} evidence section")
    start = matches[0] + 1
    end = len(lines)
    for index in range(start, len(lines)):
        if lines[index].startswith("### ") or lines[index].startswith("## "):
            end = index
            break
    section = "\n".join(lines[start:end])
    require(section.strip(), f"report {provider} evidence section is empty")
    return section


def report_agreement_fields(provider: str, row: dict[str, object]) -> dict[str, str]:
    first = row["turn_one"]
    second = row["turn_two"]
    assert isinstance(first, dict) and isinstance(second, dict)
    return {
        "Executable": str(row["executable"]),
        "Version": str(row["version"]),
        "Result": f"{row['status']} — {row['reason']}",
        "Working-directory token/digest": f"{row['cwd_token']} / {row['cwd_digest']}",
        "Observed/resumed session": f"{row['observed_session_id']} / {row['resume_session_id']}",
        "Turn-two observed session": str(row["turn_two_observed_session_id"]),
        "Turn exits": f"{first['exit_code']} / {second['exit_code']}",
        "Raw stream digests": f"{first['raw_sha256']} / {second['raw_sha256']}",
        "Normalized artifact": f"{row['event_artifact']} ({row['event_artifact_sha256']})",
        "Final file digest": str(row["final_file_sha256"]),
        "Permission behavior": str(row["permission_behavior"]),
        "Native event types": ", ".join(row["structured_event_types"]) or "none",
        "Exact native-to-normalized mapping": json.dumps(row["event_mapping"], ensure_ascii=False, sort_keys=True),
        "Unavailable/unstable fields": ", ".join(row["unavailable_fields"]) or "none",
        "Turn-one argv": json.dumps(first["argv"], ensure_ascii=False),
        "Turn-two argv": json.dumps(second["argv"], ensure_ascii=False),
    }


def report_agreement_lines(provider: str, row: dict[str, object]) -> tuple[str, ...]:
    return tuple(f"- {label}: {value}" for label, value in report_agreement_fields(provider, row).items())


def parse_provider_evidence_section(
    provider: str,
    section: str,
    expected_labels: set[str],
) -> dict[str, str]:
    fields: dict[str, str] = {}
    for number, line in enumerate(section.splitlines(), start=1):
        if line == "":
            continue
        require(line.startswith("- "), f"report {provider} evidence line {number} is malformed")
        label, delimiter, value = line[2:].partition(": ")
        require(delimiter == ": " and label and value, f"report {provider} evidence line {number} is malformed")
        require(label in expected_labels, f"report {provider} has an unexpected evidence label: {label}")
        require(label not in fields, f"report {provider} has a duplicate evidence label: {label}")
        fields[label] = value
    require(set(fields) == expected_labels, f"report {provider} evidence labels are missing or incomplete")
    return fields


def validate_report_agreement(provider: str, row: dict[str, object], section: str) -> None:
    validate_custody_digests(provider, row)
    expected = report_agreement_fields(provider, row)
    parsed = parse_provider_evidence_section(provider, section, set(expected))
    for label, value in expected.items():
        require(parsed[label] == value, f"report does not agree with {provider}: {label}")


def package_block(text: str, package: str) -> str:
    marker = f"### {package} —"
    require(marker in text, f"status ledger lacks {package}")
    return text.split(marker, 1)[1].split("\n### ", 1)[0]


def validate_plan(results: dict[str, object]) -> None:
    plan = PLAN_PATH.read_text(encoding="utf-8")
    spike = package_block(plan, "SPIKE-002")
    wp900 = package_block(plan, "WP-900")
    require("| Status | `DONE` |" in spike, "SPIKE-002 is not DONE in the status ledger")
    providers = results["providers"]
    assert isinstance(providers, dict)
    all_pass = all(providers[name]["status"] == "PASS" for name in PROVIDERS)
    if all_pass:
        require("| Status | `READY` |" in wp900, "all providers passed but WP-900 is not READY")
    else:
        require("| Status | `BLOCKED` |" in wp900, "a provider failed but WP-900 is not BLOCKED")
        failed = [name for name in PROVIDERS if providers[name]["status"] == "FAIL"]
        for provider in failed:
            require(provider in wp900.lower() or provider in plan.lower(), f"failed provider {provider} is not named in ledger")


def validate_baseline(report_path: Path) -> dict[str, object]:
    results = load_json(RESULTS_PATH)
    require(results.get("schema") == SCHEMA, "results schema mismatch")
    providers = results.get("providers")
    require(isinstance(providers, dict), "providers must be an object")
    require(set(providers) == set(PROVIDERS), "provider matrix must contain exactly codex, claude, copilot")
    rows: dict[str, dict[str, object]] = {}
    for provider in PROVIDERS:
        row, _events = validate_row(provider, providers[provider])
        rows[provider] = row
    all_pass = all(rows[name]["status"] == "PASS" for name in PROVIDERS)
    require(results.get("all_pass") is all_pass, "all_pass does not match provider rows")
    report = report_path.read_text(encoding="utf-8")
    summary = report_summary_rows(report)
    for provider in PROVIDERS:
        validate_report_summary(provider, rows[provider], summary)
        validate_report_agreement(provider, rows[provider], provider_report_section(report, provider))
    require("does not claim a production adapter or coordinator" in report, "report claim boundary missing")
    scan_candidate(results, report_path)
    validate_plan(results)
    return results


def expect_failure(action, label: str) -> None:
    try:
        action()
    except (ValidationError, json.JSONDecodeError, UnicodeError):
        return
    raise ValidationError(f"negative regression survived: {label}")


def events_for(provider: str) -> list[dict[str, object]]:
    return parse_events_bytes((EVENT_DIR / f"{provider}.jsonl").read_bytes(), provider)


def run_negative_regressions(results: dict[str, object]) -> None:
    providers = results["providers"]
    assert isinstance(providers, dict)
    claude_row = providers["claude"]
    codex_row = providers["codex"]
    copilot_row = providers["copilot"]
    assert isinstance(claude_row, dict) and isinstance(codex_row, dict) and isinstance(copilot_row, dict)
    codex_raw = (EVENT_DIR / "codex.jsonl").read_bytes()
    claude_events = events_for("claude")
    codex_events = events_for("codex")
    copilot_events = events_for("copilot")
    report = REPORT_PATH.read_text(encoding="utf-8")
    codex_turn_one = codex_row["turn_one"]
    codex_turn_two = codex_row["turn_two"]
    assert isinstance(codex_turn_one, dict) and isinstance(codex_turn_two, dict)
    codex_section = provider_report_section(report, "codex")

    raw_line = f"- Raw stream digests: {codex_turn_one['raw_sha256']} / {codex_turn_two['raw_sha256']}"
    altered_raw_line = "- Raw stream digests: sha256:" + "0" * 64 + " / sha256:" + "1" * 64
    raw_digest_substitution = report.replace(raw_line, altered_raw_line, 1)
    require(raw_digest_substitution != report, "raw digest report mutation setup failed")
    expect_failure(
        lambda: validate_report_agreement(
            "codex", codex_row, provider_report_section(raw_digest_substitution, "codex")
        ),
        "provider raw-stream digest substitution",
    )

    final_line = f"- Final file digest: {codex_row['final_file_sha256']}"
    altered_final_line = "- Final file digest: sha256:" + "2" * 64
    final_digest_substitution = report.replace(final_line, altered_final_line, 1)
    require(final_digest_substitution != report, "final digest report mutation setup failed")
    require(final_line in final_digest_substitution, "shared final digest report mutation setup failed")
    expect_failure(
        lambda: validate_report_agreement(
            "codex", codex_row, provider_report_section(final_digest_substitution, "codex")
        ),
        "provider rendered final-file digest substitution",
    )

    contradictory_duplicate = report.replace(
        codex_section, f"{codex_section}\n{altered_final_line}", 1
    )
    require(contradictory_duplicate != report, "contradictory duplicate report mutation setup failed")
    expect_failure(
        lambda: validate_report_agreement(
            "codex", codex_row, provider_report_section(contradictory_duplicate, "codex")
        ),
        "contradictory duplicate provider digest line",
    )

    identical_duplicate = report.replace(codex_section, f"{codex_section}\n{final_line}", 1)
    require(identical_duplicate != report, "identical duplicate report mutation setup failed")
    expect_failure(
        lambda: validate_report_agreement(
            "codex", codex_row, provider_report_section(identical_duplicate, "codex")
        ),
        "identical duplicate provider evidence label",
    )

    unexpected_evidence = report.replace(codex_section, f"{codex_section}\n- Extra custody field: absent", 1)
    require(unexpected_evidence != report, "unexpected evidence report mutation setup failed")
    expect_failure(
        lambda: validate_report_agreement(
            "codex", codex_row, provider_report_section(unexpected_evidence, "codex")
        ),
        "unexpected provider evidence label",
    )

    malformed_evidence = report.replace(codex_section, f"{codex_section}\n- Final file digest", 1)
    require(malformed_evidence != report, "malformed evidence report mutation setup failed")
    expect_failure(
        lambda: validate_report_agreement(
            "codex", codex_row, provider_report_section(malformed_evidence, "codex")
        ),
        "malformed provider evidence line",
    )

    missing_final = report.replace(codex_section, codex_section.replace(final_line, "", 1), 1)
    require(missing_final != report, "missing evidence report mutation setup failed")
    expect_failure(
        lambda: validate_report_agreement("codex", codex_row, provider_report_section(missing_final, "codex")),
        "missing provider evidence label",
    )

    summary_rows = report_summary_rows(report)
    codex_summary_line = "| " + " | ".join(summary_rows["codex"]) + " |"
    duplicate_summary = report.replace(codex_summary_line, f"{codex_summary_line}\n{codex_summary_line}", 1)
    require(duplicate_summary != report, "duplicate summary report mutation setup failed")
    expect_failure(lambda: report_summary_rows(duplicate_summary), "duplicate provider summary row")

    mutually_malformed_raw = copy.deepcopy(codex_row)
    malformed_turn_one = mutually_malformed_raw["turn_one"]
    assert isinstance(malformed_turn_one, dict)
    malformed_turn_one["raw_sha256"] = "sha256:not-a-digest"
    malformed_raw_report = "\n".join(report_agreement_lines("codex", mutually_malformed_raw))
    expect_failure(
        lambda: validate_report_agreement("codex", mutually_malformed_raw, malformed_raw_report),
        "mutually consistent malformed raw digest in results and report",
    )
    expect_failure(
        lambda: validate_pass_requirements("codex", mutually_malformed_raw, codex_events),
        "malformed raw digest in results",
    )

    claude_mapping = derive_event_mapping("claude", claude_events)
    require(claude_mapping.get("system.init") == "session.started", "Claude init mapping is missing")
    require(
        claude_mapping.get("system.thinking_tokens") == "provider.event",
        "Claude non-init system mapping is not provider-neutral",
    )
    require(
        all(native == "system.init" or normalized != "session.started" for native, normalized in claude_mapping.items()),
        "a non-init Claude system subtype maps to session.started",
    )
    expect_failure(
        lambda: normalized_event_type("claude", "system.unknown"),
        "unknown Claude system subtype",
    )

    expect_failure(lambda: parse_events_bytes(codex_raw[:-1] + b"{", "codex"), "corrupt JSONL")

    changed_digest = copy.deepcopy(codex_row)
    changed_digest["final_file_sha256"] = "sha256:" + "0" * 64
    expect_failure(
        lambda: validate_pass_requirements("codex", changed_digest, codex_events),
        "changed final digest",
    )

    substituted_session = copy.deepcopy(codex_row)
    substituted_session["resume_session_id"] = "session-b"
    expect_failure(
        lambda: validate_pass_requirements("codex", substituted_session, codex_events),
        "session substitution",
    )

    fabricated_session = "00000000-0000-4000-8000-000000000085"
    fabricated_evidence = copy.deepcopy(codex_row)
    fabricated_evidence["observed_session_id"] = fabricated_session
    fabricated_evidence["resume_session_id"] = fabricated_session
    fabricated_evidence["turn_two_observed_session_id"] = fabricated_session
    fabricated_first, fabricated_second = expected_pass_argvs(
        "codex", str(fabricated_evidence["executable"]), fabricated_session
    )
    fabricated_turn_one = fabricated_evidence["turn_one"]
    fabricated_turn_two = fabricated_evidence["turn_two"]
    assert isinstance(fabricated_turn_one, dict) and isinstance(fabricated_turn_two, dict)
    fabricated_turn_one["argv"] = fabricated_first
    fabricated_turn_two["argv"] = fabricated_second
    fabricated_report = "\n".join(report_agreement_lines("codex", fabricated_evidence))
    validate_report_agreement("codex", fabricated_evidence, fabricated_report)
    expect_failure(
        lambda: validate_pass_requirements("codex", fabricated_evidence, codex_events),
        "fabricated results/report session disagrees with JSONL",
    )

    missing_second_session = copy.deepcopy(codex_row)
    missing_second_session["turn_two_observed_session_id"] = ""
    expect_failure(
        lambda: validate_pass_requirements("codex", missing_second_session, codex_events),
        "missing turn-two observed session",
    )

    wrong_cwd = copy.deepcopy(codex_events)
    wrong_cwd[0]["cwd_digest"] = "sha256:" + "0" * 64
    expect_failure(lambda: validate_event_bindings("codex", codex_row, wrong_cwd), "wrong worktree")

    malformed_cwd = copy.deepcopy(codex_events)
    malformed_cwd[0]["cwd_digest"] = "sha256:not-a-digest"
    expect_failure(lambda: validate_event_bindings("codex", codex_row, malformed_cwd), "malformed JSONL cwd digest")

    stale_mapping = copy.deepcopy(codex_row)
    stale_mapping["event_mapping"] = {}
    expect_failure(
        lambda: validate_pass_requirements("codex", stale_mapping, codex_events),
        "event mapping mismatch",
    )

    relabelled_event = copy.deepcopy(codex_events)
    relabelled_event[0]["normalized_event_type"] = "turn.failed"
    expect_failure(
        lambda: validate_event_bindings("codex", codex_row, relabelled_event),
        "normalized event relabeling",
    )

    unknown_native_event = copy.deepcopy(codex_events)
    unknown_native_event[0]["native_event_type"] = "unknown.native.event"
    unknown_native_event[0]["normalized_event_type"] = "provider.event"
    expect_failure(
        lambda: validate_event_bindings("codex", codex_row, unknown_native_event),
        "unknown native event type",
    )

    bypass_argv = copy.deepcopy(copilot_row)
    bypass_first = bypass_argv["turn_one"]
    assert isinstance(bypass_first, dict)
    bypass_values = bypass_first["argv"]
    assert isinstance(bypass_values, list)
    bypass_values.insert(-1, "--allow-all-paths")
    expect_failure(
        lambda: validate_pass_requirements("copilot", bypass_argv, copilot_events),
        "Copilot path bypass flag",
    )

    with tempfile.TemporaryDirectory(prefix="spike002-negative-") as directory:
        candidate_root = Path(directory)
        for relative in EXPECTED_ARTIFACT_RELATIVE_PATHS:
            artifact = candidate_root / relative
            artifact.parent.mkdir(parents=True, exist_ok=True)
            artifact.write_text("placeholder\n", encoding="utf-8")
        (candidate_root / "unexpected.txt").write_text("unexpected\n", encoding="utf-8")
        expect_failure(lambda: validate_candidate_file_set(candidate_root), "unexpected candidate artifact")

    synthetic_path = Path("synthetic.txt")
    raw_scratch = "/" + "private" + "/" + "tmp" + "/" + "probe"
    expect_failure(
        lambda: scan_text(synthetic_path, raw_scratch, [], True),
        "raw scratch path",
    )
    secret_sentinel = "SPIKE_" + "SECRET_" + "SENTINEL"
    reasoning_sentinel = "SPIKE_" + "REASONING_" + "SENTINEL"
    expect_failure(
        lambda: scan_text(synthetic_path, secret_sentinel, [], True),
        "secret sentinel",
    )
    expect_failure(
        lambda: scan_text(synthetic_path, reasoning_sentinel, [], True),
        "reasoning sentinel",
    )

    expect_failure(
        lambda: validate_report_agreement("codex", codex_row, ""),
        "report/result agreement",
    )


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: validate_report.py docs/spikes/SPIKE-002-REPORT.md", file=sys.stderr)
        return 2
    report_path = (REPO_ROOT / sys.argv[1]).resolve() if not Path(sys.argv[1]).is_absolute() else Path(sys.argv[1]).resolve()
    require(report_path == REPORT_PATH.resolve(), "unexpected report path")
    try:
        results = validate_baseline(report_path)
        run_negative_regressions(results)
    except (OSError, ValidationError, KeyError, TypeError, ValueError) as exc:
        print(f"SPIKE-002 validation failed: {exc}", file=sys.stderr)
        return 1
    statuses = ", ".join(f"{name}={results['providers'][name]['status']}" for name in PROVIDERS)
    print(f"SPIKE-002 validation passed: {statuses}; negative regressions detected")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
