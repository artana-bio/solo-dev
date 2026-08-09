use std::path::PathBuf;

use crate::domain::assurance::ProbeKind;

pub(super) struct DisposableProbeProject {
    pub root: PathBuf,
    pub control: PathBuf,
    pub worktree: PathBuf,
}

pub(super) struct CandidateCase {
    pub delivered_sha: String,
}

pub(super) struct ReviewCase {
    pub actor: &'static str,
    pub principal: &'static str,
    pub session: &'static str,
}

impl DisposableProbeProject {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            control: root.join("control"),
            worktree: root.join("assurance-worktrees").join("F-001"),
            root,
        }
    }

    pub(super) fn probe_file(&self, kind: ProbeKind) -> PathBuf {
        if kind == ProbeKind::OutOfScopeWrite {
            self.worktree.join("README.out")
        } else {
            self.worktree.join("src/F-001/probe.txt")
        }
    }

    pub(super) fn prepare_candidate_case(
        kind: ProbeKind,
        base: &str,
        candidate: &str,
    ) -> CandidateCase {
        CandidateCase {
            delivered_sha: delivered_sha(kind, base, candidate).to_owned(),
        }
    }

    pub(super) fn prepare_review_case(kind: ProbeKind) -> ReviewCase {
        ReviewCase {
            actor: review_actor(kind),
            principal: review_principal(kind),
            session: review_session(kind),
        }
    }
}

pub(super) fn delivered_sha<'a>(kind: ProbeKind, base: &'a str, candidate: &'a str) -> &'a str {
    if kind == ProbeKind::StaleSha {
        base
    } else {
        candidate
    }
}

pub(super) fn review_actor(kind: ProbeKind) -> &'static str {
    if kind == ProbeKind::SelfReview {
        "operator"
    } else {
        "reviewer"
    }
}

pub(super) fn review_principal(kind: ProbeKind) -> &'static str {
    if kind == ProbeKind::SelfReview {
        "implementer-principal"
    } else {
        "reviewer-principal"
    }
}

pub(super) fn review_session(kind: ProbeKind) -> &'static str {
    if matches!(kind, ProbeKind::SelfReview | ProbeKind::SameSessionReview) {
        "session-implementer"
    } else {
        "reviewer-session"
    }
}
