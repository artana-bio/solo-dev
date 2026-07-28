use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::error::HarnessError;

#[derive(Debug)]
pub struct GitProbe {
    pub version: String,
    pub repository_root: Option<PathBuf>,
}

pub struct GitClient;

impl GitClient {
    /// Inspects the installed Git executable and repository containing a path.
    ///
    /// # Errors
    ///
    /// Returns an error when Git cannot be executed or its version command
    /// fails.
    pub fn probe(workspace: &Path) -> Result<GitProbe, HarnessError> {
        let version_output = Command::new("git")
            .arg("--version")
            .output()
            .map_err(HarnessError::GitUnavailable)?;

        if !version_output.status.success() {
            return Err(HarnessError::GitCommand(
                String::from_utf8_lossy(&version_output.stderr)
                    .trim()
                    .to_owned(),
            ));
        }

        let root_output = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(HarnessError::GitUnavailable)?;

        let repository_root = root_output.status.success().then(|| {
            PathBuf::from(
                String::from_utf8_lossy(&root_output.stdout)
                    .trim()
                    .to_owned(),
            )
        });

        Ok(GitProbe {
            version: String::from_utf8_lossy(&version_output.stdout)
                .trim()
                .to_owned(),
            repository_root,
        })
    }
}
