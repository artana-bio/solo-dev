use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("workspace does not exist: {0}")]
    WorkspaceNotFound(PathBuf),

    #[error("cannot access workspace {path}: {source}")]
    WorkspaceAccess {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to execute Git: {0}")]
    GitUnavailable(#[source] io::Error),

    #[error("Git command failed: {0}")]
    GitCommand(String),

    #[error("failed to encode report: {0}")]
    ReportEncoding(#[from] serde_json::Error),
}
