//! Descriptor-bound access to declared `JUnit` reports.
//!
//! Every component is opened relative to an already opened evaluation
//! worktree with `O_NOFOLLOW`. The parser consumes the returned file handle,
//! so a pathname substitution after validation cannot change the bytes read.

use std::{
    fs::File,
    io::Read as _,
    os::unix::ffi::OsStrExt as _,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use rustix::{
    fs::{CWD, Mode, OFlags, openat},
    io::Errno,
};

use crate::{
    domain::digest::Digest,
    runner::{junit::MAX_JUNIT_BYTES, receipt::StructuredResultErrorCode},
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReportBefore {
    pub(crate) length: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) digest: Digest,
}

#[derive(Debug)]
pub(crate) struct ReportFile {
    pub(crate) bytes: Vec<u8>,
    pub(crate) state: ReportBefore,
}

/// Open and read one declared path without following any path component.
pub(crate) fn read_report(
    working_directory: &Path,
    worktree: &Path,
    declared: &str,
) -> Result<ReportFile, StructuredResultErrorCode> {
    let file = open_declared_file(working_directory, worktree, declared)?;
    let metadata = file
        .metadata()
        .map_err(|_| StructuredResultErrorCode::ReadError)?;
    if !metadata.is_file() {
        return Err(StructuredResultErrorCode::UnsafePath);
    }

    let mut bytes = Vec::new();
    file.take((MAX_JUNIT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| StructuredResultErrorCode::ReadError)?;
    if bytes.len() > MAX_JUNIT_BYTES {
        return Err(StructuredResultErrorCode::Oversized);
    }

    let digest = Digest::of_bytes(&bytes);
    Ok(ReportFile {
        state: ReportBefore {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            digest,
        },
        bytes,
    })
}

/// Capture the exact pre-attempt state, if a report already exists.
pub(crate) fn capture_before(
    working_directory: &Path,
    worktree: &Path,
    declared: &str,
) -> Result<Option<ReportBefore>, StructuredResultErrorCode> {
    match read_report(working_directory, worktree, declared) {
        Ok(report) => Ok(Some(report.state)),
        Err(StructuredResultErrorCode::Missing) => Ok(None),
        Err(error) => Err(error),
    }
}

fn open_declared_file(
    working_directory: &Path,
    worktree: &Path,
    declared: &str,
) -> Result<File, StructuredResultErrorCode> {
    let worktree = worktree
        .canonicalize()
        .map_err(|_| StructuredResultErrorCode::UnsafePath)?;
    let relative_working_directory = working_directory
        .strip_prefix(&worktree)
        .map_err(|_| StructuredResultErrorCode::UnsafePath)?;
    let report = checked_relative_path(declared)?;
    let root = openat(
        CWD,
        worktree,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(map_open_error)?;

    let mut components: Vec<Vec<u8>> = relative_components(relative_working_directory)
        .chain(relative_components(&report))
        .map(component_name)
        .collect::<Result<_, _>>()?;
    let Some(last) = components.pop() else {
        return Err(StructuredResultErrorCode::Missing);
    };
    let mut directory = root;
    for name in components {
        let opened = openat(
            &directory,
            &name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(map_open_error)?;
        directory = opened;
    }
    let opened = openat(
        &directory,
        &last,
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(map_open_error)?;
    Ok(File::from(opened))
}

fn checked_relative_path(declared: &str) -> Result<PathBuf, StructuredResultErrorCode> {
    let path = Path::new(declared);
    if declared.is_empty()
        || declared.contains('\0')
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(StructuredResultErrorCode::UnsafePath);
    }
    Ok(path.to_path_buf())
}

fn relative_components(path: &Path) -> impl Iterator<Item = Component<'_>> {
    path.components()
        .filter(|component| matches!(component, Component::Normal(_)))
}

fn component_name(component: Component<'_>) -> Result<Vec<u8>, StructuredResultErrorCode> {
    match component {
        Component::Normal(name) if !name.as_bytes().contains(&0) => Ok(name.as_bytes().to_vec()),
        _ => Err(StructuredResultErrorCode::UnsafePath),
    }
}

fn map_open_error(error: Errno) -> StructuredResultErrorCode {
    if error == Errno::NOENT {
        StructuredResultErrorCode::Missing
    } else if error == Errno::LOOP || error == Errno::NOTDIR {
        StructuredResultErrorCode::UnsafePath
    } else {
        StructuredResultErrorCode::ReadError
    }
}
