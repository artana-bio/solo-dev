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
RESULTS_PATH = SCRIPT_DIR / "results.json"
EVENT_DIR = SCRIPT_DIR / "events"
PLAN_PATH = REPO_ROOT / "docs" / "IMPLEMENTATION_PLAN.md"
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


class ValidationError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValidationError(message)


def sha256_bytes(value: bytes) -> str:
    return "sha256:" + hashlib.sha256(value).hexdigest()


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
        require(isinstance(value["cwd_digest"], str) and value["cwd_digest"], f"{provider} cwd digest missing")
        events.append(value)
    return events


def validate_turn(turn: object, provider: str, label: str) -> dict[str, object]:
    require(isinstance(turn, dict), f"{provider} {label} must be an object")
    require(set(turn) == TURN_KEYS, f"{provider} {label} fields mismatch")
    require(isinstance(turn["argv"], list) and turn["argv"], f"{provider} {label} argv missing")
    require("<redacted-prompt>" in turn["argv"], f"{provider} {label} prompt was not redacted")
    require(isinstance(turn["elapsed_ms"], int) and turn["elapsed_ms"] >= 0, f"{provider} {label} elapsed invalid")
    require(isinstance(turn["timed_out"], bool), f"{provider} {label} timeout invalid")
    require(isinstance(turn["raw_sha256"], str) and turn["raw_sha256"].startswith("sha256:"), f"{provider} {label} raw digest missing")
    return turn


def validate_row(provider: str, row: object) -> tuple[dict[str, object], list[dict[str, object]]]:
    require(isinstance(row, dict), f"{provider} row must be an object")
    require(set(row) == ROW_KEYS, f"{provider} row has unknown/missing fields")
    require(row["provider"] == provider, f"{provider} row identity mismatch")
    require(row["status"] in ("PASS", "FAIL"), f"{provider} result must be PASS or FAIL")
    for field in ("executable", "version", "cwd_token", "cwd_digest", "event_artifact", "reason"):
        require(isinstance(row[field], str) and row[field].strip(), f"{provider} {field} missing")
    require(row["cwd_token"] == f"$PROBE_ROOT/{provider}", f"{provider} cwd token mismatch")
    first = validate_turn(row["turn_one"], provider, "turn_one")
    second = validate_turn(row["turn_two"], provider, "turn_two")
    event_path = REPO_ROOT / str(row["event_artifact"])
    require(event_path == EVENT_DIR / f"{provider}.jsonl", f"{provider} event path escapes expected location")
    raw = event_path.read_bytes()
    require(sha256_bytes(raw) == row["event_artifact_sha256"], f"{provider} event digest mismatch")
    events = parse_events_bytes(raw, provider)
    for event in events:
        require(event["cwd_digest"] == row["cwd_digest"], f"{provider} event cwd binding mismatch")
    if row["status"] == "PASS":
        require(first["exit_code"] == 0 and second["exit_code"] == 0, f"{provider} PASS has nonzero exit")
        require(not first["timed_out"] and not second["timed_out"], f"{provider} PASS timed out")
        require(row["structured_output"] is True and bool(events), f"{provider} PASS lacks structured events")
        observed = row["observed_session_id"]
        resumed = row["resume_session_id"]
        require(isinstance(observed, str) and observed.strip(), f"{provider} PASS lacks observed session")
        require(observed == resumed, f"{provider} PASS resumed a different session")
        second_observed = row["turn_two_observed_session_id"]
        require(second_observed in (None, resumed), f"{provider} turn-two session mismatch")
        require(row["session_continuity"] is True, f"{provider} PASS continuity is false")
        require(row["expected_content"] is True, f"{provider} PASS final content mismatch")
    else:
        require(str(row["reason"]).strip() not in ("FAIL", "unknown"), f"{provider} FAIL reason is not specific")
    require(isinstance(row["structured_event_types"], list), f"{provider} event types missing")
    require(isinstance(row["event_mapping"], dict) and row["event_mapping"], f"{provider} event mapping missing")
    require(isinstance(row["unavailable_fields"], list), f"{provider} unavailable fields missing")
    return row, events


def scan_candidate(results: dict[str, object], report_path: Path) -> None:
    secret_sentinel = "SPIKE_" + "SECRET_" + "SENTINEL"
    reasoning_sentinel = "SPIKE_" + "REASONING_" + "SENTINEL"
    home_prefix = "/" + "Users" + "/"
    token_pattern = re.compile(r"(?:sk-[A-Za-z0-9_-]{16,}|ghp_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,})")
    executable_values: list[str] = []
    providers = results.get("providers")
    if isinstance(providers, dict):
        for row in providers.values():
            if isinstance(row, dict) and isinstance(row.get("executable"), str):
                executable_values.append(row["executable"])
    paths = [report_path, RESULTS_PATH, Path(__file__), SCRIPT_DIR / "run_probe.py"]
    paths.extend(sorted(EVENT_DIR.glob("*.jsonl")))
    for path in paths:
        text = path.read_text(encoding="utf-8")
        scrubbed = text
        for executable in executable_values:
            scrubbed = scrubbed.replace(executable, "<allowed-executable>")
        require(secret_sentinel not in scrubbed, f"forbidden secret sentinel in {path.name}")
        require(reasoning_sentinel not in scrubbed, f"forbidden reasoning sentinel in {path.name}")
        require(home_prefix not in scrubbed, f"unrestricted home path in {path.name}")
        require(not token_pattern.search(scrubbed), f"token-like value in {path.name}")
        if path in (report_path, RESULTS_PATH) or path.suffix == ".jsonl":
            lowered = scrubbed.lower()
            require('"stdout"' not in lowered and '"stderr"' not in lowered, f"raw stream field in {path.name}")
            require('"reasoning"' not in lowered and '"message"' not in lowered and '"content"' not in lowered, f"free-form payload field in {path.name}")


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
    for provider in PROVIDERS:
        expected = f"| {provider} | {rows[provider]['status']} |"
        require(expected in report, f"report matrix does not match {provider} result")
        require(str(rows[provider]["event_artifact_sha256"]) in report, f"report omits {provider} artifact digest")
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


def run_negative_regressions(results: dict[str, object]) -> None:
    provider = PROVIDERS[0]
    raw = (EVENT_DIR / f"{provider}.jsonl").read_bytes()
    expect_failure(lambda: parse_events_bytes(raw[:-1] + b"{", provider), "corrupt JSONL")
    synthetic = {
        "observed_session_id": "session-a",
        "resume_session_id": "session-b",
        "status": "PASS",
    }
    expect_failure(
        lambda: require(synthetic["observed_session_id"] == synthetic["resume_session_id"], "session mismatch"),
        "session substitution",
    )
    events = parse_events_bytes(raw, provider)
    if not events:
        events = [{"cwd_digest": results["providers"][provider]["cwd_digest"]}]
    mutated = copy.deepcopy(events)
    mutated[0]["cwd_digest"] = "sha256:" + "0" * 64
    expected_cwd = results["providers"][provider]["cwd_digest"]
    expect_failure(
        lambda: [require(event["cwd_digest"] == expected_cwd, "cwd mismatch") for event in mutated],
        "wrong worktree",
    )
    secret_sentinel = "SPIKE_" + "SECRET_" + "SENTINEL"
    reasoning_sentinel = "SPIKE_" + "REASONING_" + "SENTINEL"
    with tempfile.TemporaryDirectory(prefix="spike002-negative-") as directory:
        marker = Path(directory) / "marker.txt"
        for label, sentinel in (("secret sentinel", secret_sentinel), ("reasoning sentinel", reasoning_sentinel)):
            marker.write_text(sentinel, encoding="utf-8")
            expect_failure(lambda value=sentinel: require(value not in marker.read_text(encoding="utf-8"), label), label)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: validate_report.py docs/spikes/SPIKE-002-REPORT.md", file=sys.stderr)
        return 2
    report_path = (REPO_ROOT / sys.argv[1]).resolve() if not Path(sys.argv[1]).is_absolute() else Path(sys.argv[1]).resolve()
    require(report_path == (REPO_ROOT / "docs" / "spikes" / "SPIKE-002-REPORT.md").resolve(), "unexpected report path")
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
