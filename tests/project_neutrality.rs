//! D-002: the engine is project- and language-neutral.
//!
//! Section 19.4 tests this with an ARTANA profile trial, which needs a second
//! repository. Nothing else tested it at all, which meant a Rust or cargo
//! assumption could be baked into the engine and every suite would still pass —
//! because every fixture, and the harness's own self-hosted development, is a
//! Rust project checked by cargo.
//!
//! This drives a complete lifecycle against a repository containing no Rust,
//! with gates that are ordinary shell commands. It does not substitute for the
//! ARTANA trial, which tests a real profile against real constraints; it
//! catches the narrower failure of the engine having learned something about
//! its own language.

mod support;

use std::{fs, process::Command};

use support::Workspace;

/// Writes a small Python project into the candidate repository.
fn python_project(workspace: &Workspace) {
    fs::create_dir_all(workspace.repository.join("app")).unwrap();
    fs::write(
        workspace.repository.join("app/__init__.py"),
        "\"\"\"A project the harness knows nothing about.\"\"\"\n",
    )
    .unwrap();
    fs::write(
        workspace.repository.join("app/greet.py"),
        "def greet(name):\n    return f\"hello, {name}\"\n",
    )
    .unwrap();
    fs::write(
        workspace.repository.join("Makefile"),
        "check:\n\tpython3 -m compileall -q app\n",
    )
    .unwrap();
    // A real Python project ignores its bytecode cache, and it has to: the
    // compile gate writes `__pycache__` into the worktree, and an untracked
    // file blocks handoff by design — an untracked file can silently become
    // part of a candidate. Without this the lifecycle cannot complete, which
    // is a constraint no Rust-only suite would ever surface, because this
    // repository ignores `target/` and nobody had to think about it.
    fs::write(workspace.repository.join(".gitignore"), "__pycache__/\n").unwrap();
    support::git(&workspace.repository, &["add", "-A"]);
    support::git(
        &workspace.repository,
        &["commit", "-q", "-m", "add the python project"],
    );
    support::git(
        &workspace.repository,
        &["push", "-q", "harness-authority", "main"],
    );
}

#[test]
fn a_project_with_no_rust_completes_the_whole_lifecycle() {
    let workspace = Workspace::initialized();
    python_project(&workspace);

    // Gates that are this project's actual checks. Nothing here mentions
    // cargo, and the harness has no idea what `python3` is.
    workspace.register_gate("py.compile", &["python3", "-m", "compileall", "-q", "app"]);
    workspace.register_gate("py.smoke", &["make", "check"]);

    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Add a farewell",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gate_sets("F-001", &["app/**"], &["py.compile"], &["py.smoke"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::write(
        worktree.join("app/farewell.py"),
        "def farewell(name):\n    return f\"goodbye, {name}\"\n",
    )
    .unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add farewell"]);

    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "py.compile"]);

    let delivered = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {delivered}\nbehavior_delivered: adds farewell\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);

    workspace.review(&["begin", "--card-id", "F-001"]);
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: ran the compile gate against a deliberate syntax error\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n",
    )
    .unwrap();
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict.display().to_string(),
    ]);

    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "integration-reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
    ]);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    workspace.archive(&["create", "--integration-id", &id]);
    workspace.archive(&["close", "--integration-id", &id]);

    // The change is on the protected branch, and it is Python.
    let listing = support::capture(
        &workspace.repository,
        &["ls-tree", "-r", "--name-only", &workspace.authority_head()],
    );
    assert!(listing.contains("app/farewell.py"), "unexpected: {listing}");
    assert!(
        !listing.contains("Cargo.toml"),
        "the fixture must genuinely not be a Rust project: {listing}"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "closed"
    );
}

#[test]
fn a_failing_gate_in_another_language_blocks_exactly_as_a_rust_one_would() {
    // The interesting half: the engine must not only run a foreign gate, it
    // must read its verdict the same way. A gate that fails has to block.
    let workspace = Workspace::initialized();
    python_project(&workspace);
    workspace.register_gate("py.compile", &["python3", "-m", "compileall", "-q", "app"]);

    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Add a farewell",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gate_sets("F-001", &["app/**"], &["py.compile"], &["gate.all"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("app/broken.py"), "def broken(:\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: syntax error"]);

    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "py.compile",
        "--actor",
        "operator",
    ]);
    let reservation_id = reservation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let output = workspace.gate_raw(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "py.compile",
        "--reservation-id",
        reservation_id,
        "--actor",
        "operator",
    ]);
    assert_eq!(
        output.status.code(),
        Some(7),
        "a failing foreign gate must land in the gate category like any other"
    );

    // And the receipt records the failure rather than the run being forgotten.
    let failed = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "gate.ran")
        .expect("the run must be recorded");
    assert_eq!(failed["metadata"]["passed"], false);
}

#[test]
fn language_neutral_configuration_still_guards_a_python_dependency_manifest() {
    // The project document is the only place a project describes itself to the
    // harness. If neutrality is real, nothing in it names a language, a build
    // tool, or a file extension.
    let workspace = Workspace::initialized();
    python_project(&workspace);

    let document =
        fs::read_to_string(workspace.control.join("project/project.json")).expect("a project doc");
    for term in ["cargo", "rust", "python", ".rs", ".py", "npm", "make"] {
        assert!(
            !document.to_lowercase().contains(term),
            "the project document names `{term}`, so the engine is not neutral: {document}"
        );
    }

    // Nor does the binary need to know: the gate registry is where a project's
    // commands live, and it takes whatever argv it is given.
    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["--help"])
        .output()
        .expect("the CLI should start");
    let help = String::from_utf8_lossy(&output.stdout).to_lowercase();
    for term in ["cargo", "rust", "python"] {
        assert!(
            !help.contains(term),
            "the command surface names `{term}`: {help}"
        );
    }

    // A non-Rust dependency manifest must receive the same supply-chain guard
    // without adding a language-specific setting to the project document.
    // First prove this broad scope is otherwise usable, so the refusal below
    // cannot be an unrelated scope or worktree failure.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Update dependencies",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    fs::write(
        worktree.join("app/neutral.txt"),
        "ordinary in-scope change\n",
    )
    .unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(
        &worktree,
        &[
            "commit",
            "-q",
            "-m",
            "test: prove broad scope is admissible",
        ],
    );
    let ordinary = workspace.work_json(&["verify", "--card-id", "F-001"]);
    assert_eq!(
        ordinary["data"]["passed"], true,
        "the fixture must admit an ordinary Python-project path: {:?}",
        ordinary["data"]["findings"]
    );

    fs::write(worktree.join("app/requirements.txt"), "example==1.0\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(
        &worktree,
        &["commit", "-q", "-m", "build: update Python dependencies"],
    );
    let guarded = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(
        guarded.status.code(),
        Some(5),
        "an undeclared foreign dependency manifest must be refused: {}{}",
        String::from_utf8_lossy(&guarded.stdout),
        String::from_utf8_lossy(&guarded.stderr)
    );
    let refusal = String::from_utf8_lossy(&guarded.stdout);
    assert!(
        refusal.contains(
            "`app/requirements.txt` changes what the build resolves and must be named explicitly"
        ),
        "the refusal must come from the dependency-manifest policy, not an unrelated check: {refusal}"
    );
}
