//! Gate execution and receipts.
//!
//! This is the only module that runs code the harness did not write. Section
//! 14.1 governs it: the executable is invoked directly with no shell, the
//! environment is an allowlist rather than an inheritance, and a timeout must
//! terminate the whole process group rather than only the immediate child.
//!
//! The process-group rule is not pedantry. A test runner that spawns workers
//! and is killed alone leaves those workers holding ports, files, and CPU, and
//! the next gate run then fails for reasons that have nothing to do with the
//! code under test.

pub(crate) mod junit;
pub mod receipt;

use std::{
    fs::File,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::{
    domain::{
        clock::Clock,
        digest::Digest,
        gate::{GateDefinition, NetworkPolicy},
    },
    error::{ErrorCode, HarnessError},
};

use receipt::{Termination, attempt_log_paths};

/// Longest log this runner retains per stream, in bytes.
///
/// Section 14.1 requires bounded logs. A gate that prints without limit would
/// otherwise fill the disk, and the tail is where failures usually are, so the
/// tail is what is kept.
pub const MAX_LOG_BYTES: usize = 1_048_576;

/// How long a terminated process group is given to exit before it is killed.
const GRACE_PERIOD: Duration = Duration::from_secs(5);

/// How often a running gate is polled for completion.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The outcome of one gate attempt, before it becomes a receipt.
#[derive(Clone, Debug)]
pub struct AttemptOutcome {
    /// Which attempt this was, starting at 1.
    pub attempt: u32,
    /// The process exit code, absent when it was signalled.
    pub exit_code: Option<i32>,
    /// How the process ended.
    pub termination: Termination,
    /// Digest of the captured standard output.
    pub stdout_digest: Digest,
    /// Digest of the captured standard error.
    pub stderr_digest: Digest,
    /// Where the logs were written.
    pub log_location: PathBuf,
    /// How long the attempt took.
    pub duration_ms: u64,
    /// Digests of declared artifacts that were produced.
    pub artifact_digests: std::collections::BTreeMap<String, String>,
    /// Validated counts from reports declared by the gate, if any.
    pub test_results: Option<receipt::TestResultSummary>,
}

impl AttemptOutcome {
    /// True when the gate passed.
    ///
    /// Section 14.4: success requires exit zero *and* completion before the
    /// timeout. A process killed at its deadline can still exit zero on some
    /// platforms, so both conditions are checked.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.termination == Termination::Completed
            && self.exit_code == Some(0)
            && self
                .test_results
                .as_ref()
                .is_none_or(|results| results.status != receipt::TestResultStatus::Invalid)
    }
}

/// Describes the environment a gate ran under, for reproducibility.
///
/// Recorded rather than trusted: two runs that disagree can be compared, which
/// is impossible if the environment is not part of the evidence.
#[must_use]
pub fn environment_fingerprint(gate: &GateDefinition) -> String {
    let mut parts = vec![
        format!("os={}", std::env::consts::OS),
        format!("arch={}", std::env::consts::ARCH),
        format!("harness={}", env!("CARGO_PKG_VERSION")),
        format!("network={:?}", gate.network_policy),
        // Recorded beside the declaration rather than left to the reader. A
        // receipt saying only `network=Denied` describes what the gate asked
        // for as though it were what the runner imposed, which is the one
        // thing Section 14.1 says evidence must never do.
        format!("network_enforced={}", NetworkPolicy::ENFORCED),
    ];
    let mut allowed: Vec<&String> = gate.environment.allow.iter().collect();
    allowed.sort();
    parts.push(format!(
        "allow=[{}]",
        allowed
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>()
            .join(",")
    ));
    parts.join(" ")
}

/// Runs one gate attempt inside an evaluation worktree.
///
/// # Errors
///
/// Returns an error when the working directory is invalid or the process could
/// not be spawned at all. A gate that runs and fails is not an error here; it
/// is a recorded outcome.
pub fn run_attempt(
    gate: &GateDefinition,
    worktree: &Path,
    log_root: &Path,
    attempt: u32,
    clock: &dyn Clock,
) -> Result<AttemptOutcome, HarnessError> {
    run_attempt_with_validation_cache(gate, worktree, log_root, attempt, clock, None)
}

/// Runs one attempt with an optional private validation cache supplied by the
/// execution coordinator. The override is installed after gate configuration,
/// so a gate definition cannot redirect a governed run to shared state.
///
/// # Errors
///
/// Returns the same runner and filesystem errors as [`run_attempt`].
#[allow(clippy::too_many_lines)]
pub fn run_attempt_with_validation_cache(
    gate: &GateDefinition,
    worktree: &Path,
    log_root: &Path,
    attempt: u32,
    clock: &dyn Clock,
    validation_cache: Option<&Path>,
) -> Result<AttemptOutcome, HarnessError> {
    let working_directory = resolve_working_directory(gate, worktree)?;
    let report_before = junit::capture_before(gate, &working_directory, worktree, attempt);
    let (stdout_path, stderr_path) = attempt_log_paths(log_root, &gate.gate_id, attempt);
    if let Some(parent) = stdout_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HarnessError::ControlIo {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut command = Command::new(&gate.argv[0]);
    command.args(&gate.argv[1..]);
    command.current_dir(&working_directory);

    // Deny by default: the child starts with nothing and receives only what the
    // gate names. Section 14.1 requires the candidate process not to inherit
    // production credentials, and inheriting nothing is the only way to be sure.
    command.env_clear();
    for name in &gate.environment.allow {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    for (name, value) in &gate.environment.set {
        command.env(name, value);
    }
    if let Some(cache) = validation_cache {
        command.env("CHANGE_HARNESS_VALIDATION_CACHE", cache);
    }

    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    // Its own process group, so a timeout can terminate the whole tree rather
    // than orphaning whatever the gate spawned.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }

    let started = Instant::now();
    let started_at = clock.now();
    let mut child = command.spawn().map_err(|source| HarnessError::Control {
        reason: format!(
            "gate `{}` could not start `{}`: {source}",
            gate.gate_id, gate.argv[0]
        ),
        code: ErrorCode::GateRunnerError,
    })?;
    let pid = child.id();

    // The streams must be drained while the process runs, not after it exits.
    // A pipe holds roughly 64 KiB; a gate that prints more than that would
    // block forever waiting for a reader, and the only thing that would end it
    // is its own timeout. Reading concurrently is what makes a chatty gate work
    // rather than mysteriously hang.
    //
    // Results come back over a channel rather than from `join`, because a join
    // cannot be given a deadline and this one has to have one. See the drain
    // below.
    let (sender, receiver) = std::sync::mpsc::channel::<(bool, Vec<u8>)>();
    let mut streams = 0usize;
    if let Some(stream) = child.stdout.take() {
        streams += 1;
        let sender = sender.clone();
        std::thread::spawn(move || sender.send((true, read_bounded(stream))));
    }
    if let Some(stream) = child.stderr.take() {
        streams += 1;
        let sender = sender.clone();
        std::thread::spawn(move || sender.send((false, read_bounded(stream))));
    }
    drop(sender);

    let deadline = Duration::from_secs(gate.timeout_seconds);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(source) => {
                return Err(HarnessError::Control {
                    reason: format!("gate `{}` could not be waited on: {source}", gate.gate_id),
                    code: ErrorCode::GateRunnerError,
                });
            }
        }
        if started.elapsed() >= deadline {
            timed_out = true;
            terminate_group(pid);
            break child.wait().ok();
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let drained = drain(
        &receiver,
        streams,
        pid,
        deadline.saturating_sub(started.elapsed()),
    );
    timed_out |= drained.overran;
    let (stdout, stderr) = (drained.stdout, drained.stderr);
    write_log(&stdout_path, &stdout)?;
    write_log(&stderr_path, &stderr)?;

    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let exit_code = status.as_ref().and_then(std::process::ExitStatus::code);
    let termination = classify(timed_out, status.as_ref(), exit_code);
    let artifact_digests = digest_artifacts(gate, &working_directory);
    let test_results = junit::collect(gate, &working_directory, worktree, &report_before, attempt);
    let _ = started_at;

    Ok(AttemptOutcome {
        attempt,
        exit_code,
        termination,
        // Digests are computed after the logs are closed, per Section 14.1, so
        // the digest describes exactly the bytes on disk.
        stdout_digest: Digest::of_bytes(&stdout),
        stderr_digest: Digest::of_bytes(&stderr),
        log_location: stdout_path
            .parent()
            .map_or_else(|| log_root.to_path_buf(), Path::to_path_buf),
        duration_ms,
        artifact_digests,
        test_results,
    })
}

/// Decides how a finished process ended.
fn classify(
    timed_out: bool,
    status: Option<&std::process::ExitStatus>,
    exit_code: Option<i32>,
) -> Termination {
    if timed_out {
        return Termination::Timeout;
    }
    match status {
        None => Termination::RunnerError,
        // No exit code on Unix means the process was terminated by a signal.
        Some(_) if exit_code.is_none() => Termination::Signal,
        Some(_) => Termination::Completed,
    }
}

/// What the reader threads produced, and whether waiting for them overran.
struct Drained {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    overran: bool,
}

/// Collects both output streams under a deadline.
///
/// A child exiting does not close its pipes: anything it spawned inherited
/// them, and a reader waiting on end-of-file waits for that too. This was an
/// unbounded `join` on the reasoning that the pipes are closed once the process
/// ends, which holds for the direct child and for nothing else. `sh -c 'sleep
/// 30 & exit 0'` under a one-second gate therefore returned after thirty
/// seconds and was recorded as a clean pass, since the timeout flag was only
/// ever set by the wait loop. The declared deadline has to bound the whole
/// attempt.
fn drain(
    receiver: &std::sync::mpsc::Receiver<(bool, Vec<u8>)>,
    streams: usize,
    pid: u32,
    mut remaining: Duration,
) -> Drained {
    let mut result = Drained {
        stdout: Vec::new(),
        stderr: Vec::new(),
        overran: false,
    };
    let started = Instant::now();

    for _ in 0..streams {
        let Ok((is_stdout, data)) = receiver.recv_timeout(remaining) else {
            // Past the deadline with a stream still open: something in the
            // gate's process group is holding it. Killing the group closes it,
            // which is what putting the gate in its own group is for.
            result.overran = true;
            terminate_group(pid);
            // Whatever arrives now arrives quickly. If something escaped the
            // kill, its output is lost rather than waited on forever — an
            // unbounded wait here is the defect this exists to remove.
            while let Ok((is_stdout, data)) = receiver.recv_timeout(GRACE_PERIOD) {
                result.assign(is_stdout, data);
            }
            return result;
        };
        result.assign(is_stdout, data);
        remaining = remaining.saturating_sub(started.elapsed());
    }
    result
}

impl Drained {
    fn assign(&mut self, is_stdout: bool, data: Vec<u8>) {
        if is_stdout {
            self.stdout = data;
        } else {
            self.stderr = data;
        }
    }
}

/// Terminates a process group, escalating if it does not exit.
///
/// `unsafe_code` is forbidden in this crate, so `killpg` is not available.
/// Invoking `kill` with a negative process id is the same operation expressed
/// through a process boundary, and it keeps the crate free of unsafe blocks.
/// See D-035.
fn terminate_group(pid: u32) {
    let group = format!("-{pid}");
    let _ = Command::new("kill")
        .args(["-TERM", &group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let deadline = Instant::now() + GRACE_PERIOD;
    while Instant::now() < deadline {
        let alive = Command::new("kill")
            .args(["-0", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    let _ = Command::new("kill")
        .args(["-KILL", &group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Reads a stream to end, retaining at most [`MAX_LOG_BYTES`] of its tail.
///
/// Memory is bounded while reading rather than by truncating at the end, so a
/// gate that prints gigabytes cannot exhaust the harness before its output is
/// discarded anyway.
fn read_bounded<R: std::io::Read>(mut stream: R) -> Vec<u8> {
    let mut retained: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                retained.extend_from_slice(&chunk[..read]);
                if retained.len() > MAX_LOG_BYTES * 2 {
                    // Keep the tail: a failing gate's diagnosis is almost always
                    // at the end of its output.
                    let start = retained.len() - MAX_LOG_BYTES;
                    retained.drain(..start);
                }
            }
        }
    }
    if retained.len() > MAX_LOG_BYTES {
        let start = retained.len() - MAX_LOG_BYTES;
        retained.drain(..start);
    }
    retained
}

/// Writes one captured stream to disk.
fn write_log(path: &Path, contents: &[u8]) -> Result<(), HarnessError> {
    let mut file = File::create(path).map_err(|source| HarnessError::ControlIo {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|source| HarnessError::ControlIo {
            path: path.to_path_buf(),
            source,
        })
}

/// Resolves and bounds a gate's working directory.
fn resolve_working_directory(
    gate: &GateDefinition,
    worktree: &Path,
) -> Result<PathBuf, HarnessError> {
    let resolved = if gate.working_directory.is_empty() || gate.working_directory == "." {
        worktree.to_path_buf()
    } else {
        worktree.join(&gate.working_directory)
    };

    // Re-checked here even though registration validated it, because the
    // registry stores text and this is where it becomes a real path.
    let canonical_root = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
    if !canonical.starts_with(&canonical_root) {
        return Err(HarnessError::Control {
            reason: format!(
                "gate `{}` working directory {} resolves outside the evaluation worktree {}",
                gate.gate_id,
                canonical.display(),
                canonical_root.display()
            ),
            code: ErrorCode::ConfigInvalidGate,
        });
    }
    if !canonical.is_dir() {
        return Err(HarnessError::Control {
            reason: format!(
                "gate `{}` working directory {} does not exist",
                gate.gate_id,
                canonical.display()
            ),
            code: ErrorCode::ConfigInvalidGate,
        });
    }
    Ok(canonical)
}

/// Digests every declared artifact that was actually produced.
fn digest_artifacts(
    gate: &GateDefinition,
    working_directory: &Path,
) -> std::collections::BTreeMap<String, String> {
    let mut digests = std::collections::BTreeMap::new();
    for artifact in &gate.artifacts {
        let path = working_directory.join(artifact);
        if let Ok(bytes) = std::fs::read(&path) {
            digests.insert(
                artifact.clone(),
                Digest::of_bytes(&bytes).as_str().to_owned(),
            );
        }
    }
    digests
}

/// True when the gate declares it needs no network.
#[must_use]
pub const fn declares_network_denied(gate: &GateDefinition) -> bool {
    matches!(gate.network_policy, NetworkPolicy::Denied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        clock::FixedClock,
        gate::{GATE_SCHEMA, GateEnvironment, RetryPolicy},
    };

    fn clock() -> FixedClock {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap()
    }

    fn gate(argv: &[&str]) -> GateDefinition {
        GateDefinition {
            schema: GATE_SCHEMA.to_owned(),
            gate_id: "gate.test".to_owned(),
            revision: 1,
            argv: argv.iter().map(|value| (*value).to_owned()).collect(),
            working_directory: ".".to_owned(),
            timeout_seconds: 30,
            environment: GateEnvironment::default(),
            network_policy: NetworkPolicy::Denied,
            retry_policy: RetryPolicy::default(),
            artifacts: vec![],
            junit_reports: vec![],
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let logs = temp.path().join("logs");
        std::fs::create_dir_all(&worktree).unwrap();
        (temp, worktree, logs)
    }

    #[test]
    fn a_passing_gate_is_recorded_as_completed_with_exit_zero() {
        let (_temp, worktree, logs) = fixture();
        let outcome = run_attempt(&gate(&["true"]), &worktree, &logs, 1, &clock()).unwrap();
        assert_eq!(outcome.termination, Termination::Completed);
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.passed());
    }

    #[test]
    fn a_failing_gate_is_completed_but_not_passed() {
        let (_temp, worktree, logs) = fixture();
        let outcome = run_attempt(&gate(&["false"]), &worktree, &logs, 1, &clock()).unwrap();
        assert_eq!(
            outcome.termination,
            Termination::Completed,
            "a clean non-zero exit is a completed run, not a runner failure"
        );
        assert_eq!(outcome.exit_code, Some(1));
        assert!(!outcome.passed());
    }

    #[test]
    fn a_timeout_is_distinct_from_a_failure() {
        let (_temp, worktree, logs) = fixture();
        let mut slow = gate(&["sleep", "30"]);
        slow.timeout_seconds = 1;

        let outcome = run_attempt(&slow, &worktree, &logs, 1, &clock()).unwrap();
        assert_eq!(outcome.termination, Termination::Timeout);
        assert!(!outcome.passed());
        assert!(
            outcome.duration_ms < 20_000,
            "the gate must be terminated near its deadline, took {}ms",
            outcome.duration_ms
        );
    }

    #[test]
    fn a_missing_executable_is_a_runner_error_not_a_gate_failure() {
        let (_temp, worktree, logs) = fixture();
        let error = run_attempt(
            &gate(&["definitely-not-an-executable-anywhere"]),
            &worktree,
            &logs,
            1,
            &clock(),
        )
        .expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::GateRunnerError);
    }

    #[test]
    fn stdout_and_stderr_are_captured_separately_and_digested() {
        let (_temp, worktree, logs) = fixture();
        let outcome = run_attempt(
            &gate(&["sh", "-c", "echo out; echo err >&2"]),
            &worktree,
            &logs,
            1,
            &clock(),
        )
        .unwrap();

        // The runner never uses a shell itself; `sh` here is the gate's own
        // declared executable, which is a different thing entirely.
        assert_eq!(outcome.stdout_digest, Digest::of_bytes(b"out\n"));
        assert_eq!(outcome.stderr_digest, Digest::of_bytes(b"err\n"));

        let (stdout_path, stderr_path) = attempt_log_paths(&logs, "gate.test", 1);
        assert_eq!(std::fs::read_to_string(stdout_path).unwrap(), "out\n");
        assert_eq!(std::fs::read_to_string(stderr_path).unwrap(), "err\n");
    }

    #[test]
    fn no_unallowlisted_variable_reaches_the_child() {
        let (_temp, worktree, logs) = fixture();
        // HOME is set in the parent of any realistic test run, and the gate
        // below does not allow it. Using an existing variable rather than one
        // this test sets keeps the crate free of unsafe `set_var`.
        assert!(std::env::var("HOME").is_ok(), "fixture assumes HOME is set");

        let mut probing = gate(&["sh", "-c", "echo \"${HOME:-absent}\""]);
        probing.environment.allow = vec!["PATH".to_owned()];

        let outcome = run_attempt(&probing, &worktree, &logs, 1, &clock()).unwrap();
        let (stdout_path, _) = attempt_log_paths(&logs, "gate.test", 1);
        assert_eq!(
            std::fs::read_to_string(stdout_path).unwrap(),
            "absent\n",
            "the child starts with an empty environment and receives only what the gate names"
        );
        assert_eq!(outcome.exit_code, Some(0));
    }

    #[test]
    fn an_allowlisted_variable_does_reach_the_child() {
        let (_temp, worktree, logs) = fixture();
        let expected = std::env::var("HOME").expect("fixture assumes HOME is set");

        let mut allowing = gate(&["sh", "-c", "echo \"${HOME:-absent}\""]);
        allowing.environment.allow = vec!["HOME".to_owned(), "PATH".to_owned()];

        run_attempt(&allowing, &worktree, &logs, 1, &clock()).unwrap();
        let (stdout_path, _) = attempt_log_paths(&logs, "gate.test", 1);
        assert_eq!(
            std::fs::read_to_string(stdout_path).unwrap(),
            format!("{expected}\n")
        );
    }

    #[test]
    fn a_set_variable_overrides_the_parent() {
        let (_temp, worktree, logs) = fixture();
        let mut setting = gate(&["sh", "-c", "echo \"$CH_TEST_SET\""]);
        setting
            .environment
            .set
            .insert("CH_TEST_SET".to_owned(), "fixed".to_owned());
        setting.environment.allow = vec!["PATH".to_owned()];

        run_attempt(&setting, &worktree, &logs, 1, &clock()).unwrap();
        let (stdout_path, _) = attempt_log_paths(&logs, "gate.test", 1);
        assert_eq!(std::fs::read_to_string(stdout_path).unwrap(), "fixed\n");
    }

    #[test]
    fn logs_are_bounded_and_keep_the_tail() {
        let (_temp, worktree, logs) = fixture();
        let mut noisy = gate(&[
            "sh",
            "-c",
            "head -c 2000000 /dev/zero | tr '\\0' 'a'; echo END",
        ]);
        noisy.environment.allow = vec!["PATH".to_owned()];
        noisy.timeout_seconds = 30;

        run_attempt(&noisy, &worktree, &logs, 1, &clock()).unwrap();
        let (stdout_path, _) = attempt_log_paths(&logs, "gate.test", 1);
        let captured = std::fs::read(stdout_path).unwrap();
        assert!(
            captured.len() <= MAX_LOG_BYTES,
            "logs must be bounded, got {} bytes",
            captured.len()
        );
        assert!(
            captured.ends_with(b"END\n"),
            "the tail must be kept, because that is where failures are"
        );
    }

    #[test]
    fn attempts_write_to_distinct_log_files() {
        let (_temp, worktree, logs) = fixture();
        run_attempt(&gate(&["true"]), &worktree, &logs, 1, &clock()).unwrap();
        run_attempt(&gate(&["true"]), &worktree, &logs, 2, &clock()).unwrap();

        let (first, _) = attempt_log_paths(&logs, "gate.test", 1);
        let (second, _) = attempt_log_paths(&logs, "gate.test", 2);
        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
    }

    #[test]
    fn declared_artifacts_are_digested_when_produced() {
        let (_temp, worktree, logs) = fixture();
        let mut producing = gate(&["sh", "-c", "printf report > report.txt"]);
        producing.environment.allow = vec!["PATH".to_owned()];
        producing.artifacts = vec!["report.txt".to_owned(), "absent.txt".to_owned()];

        let outcome = run_attempt(&producing, &worktree, &logs, 1, &clock()).unwrap();
        assert_eq!(
            outcome.artifact_digests.get("report.txt"),
            Some(&Digest::of_bytes(b"report").as_str().to_owned())
        );
        assert!(
            !outcome.artifact_digests.contains_key("absent.txt"),
            "an artifact that was not produced is simply absent"
        );
    }

    #[test]
    fn a_working_directory_outside_the_worktree_is_refused() {
        let (temp, worktree, logs) = fixture();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        let mut escaping = gate(&["true"]);
        escaping.working_directory = "../outside".to_owned();
        let error = run_attempt(&escaping, &worktree, &logs, 1, &clock()).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::ConfigInvalidGate);
    }

    #[test]
    fn a_subdirectory_working_directory_is_used() {
        let (_temp, worktree, logs) = fixture();
        std::fs::create_dir_all(worktree.join("sub")).unwrap();
        std::fs::write(worktree.join("sub/marker"), "here").unwrap();

        let mut nested = gate(&["sh", "-c", "cat marker"]);
        nested.working_directory = "sub".to_owned();
        nested.environment.allow = vec!["PATH".to_owned()];

        run_attempt(&nested, &worktree, &logs, 1, &clock()).unwrap();
        let (stdout_path, _) = attempt_log_paths(&logs, "gate.test", 1);
        assert_eq!(std::fs::read_to_string(stdout_path).unwrap(), "here");
    }

    #[test]
    fn the_environment_fingerprint_records_what_was_allowed() {
        let mut described = gate(&["true"]);
        described.environment.allow = vec!["PATH".to_owned(), "HOME".to_owned()];
        let fingerprint = environment_fingerprint(&described);
        assert!(fingerprint.contains("os="));
        assert!(fingerprint.contains("harness="));
        assert!(
            fingerprint.contains("allow=[HOME,PATH]"),
            "the allowlist is sorted so the fingerprint is stable: {fingerprint}"
        );
    }

    #[test]
    fn the_fingerprint_never_records_a_declared_policy_without_its_enforcement() {
        // A receipt is evidence. `network=Denied` alone reads as a control the
        // runner applied, and the runner applies nothing, so the enforcement
        // status travels with the declaration rather than being inferred.
        let denied = gate(&["true"]);
        let fingerprint = environment_fingerprint(&denied);
        assert!(
            fingerprint.contains("network=Denied"),
            "the declaration is still recorded: {fingerprint}"
        );
        assert!(
            fingerprint.contains("network_enforced=false"),
            "and it is never recorded alone: {fingerprint}"
        );

        let mut allowed = gate(&["true"]);
        allowed.network_policy = NetworkPolicy::Allowed;
        assert!(
            environment_fingerprint(&allowed).contains("network_enforced=false"),
            "the qualification does not depend on which policy was declared"
        );
    }
}
