//! Release workflow and installer regression coverage.

use std::{
    env, fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TAG: &str = "v9.9.9";
const TARGET: &str = "aarch64-apple-darwin";

struct FakeRelease {
    _temp: TempDir,
    root: PathBuf,
    tools: PathBuf,
    install_dir: PathBuf,
    asset_name: String,
}

impl FakeRelease {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary release root");
        let root = temp.path().to_path_buf();
        let tools = root.join("tools");
        let install_dir = root.join("installed");
        fs::create_dir(&tools).expect("fake tool directory");
        create_fake_tools(&tools);

        let asset_name = create_fake_archive(&root);
        write_fake_release_response(&root, &asset_name);

        Self {
            _temp: temp,
            root,
            tools,
            install_dir,
            asset_name,
        }
    }

    fn run(&self, os: &str, arch: &str) -> Output {
        let path = env::join_paths(
            std::iter::once(self.tools.clone())
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
        )
        .expect("test PATH");
        let installer =
            fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh")).expect("installer");
        let mut child = Command::new("sh")
            .env("PATH", path)
            .env("FAKE_RELEASE_ROOT", &self.root)
            .env("FAKE_UNAME_S", os)
            .env("FAKE_UNAME_M", arch)
            .env("HOME", self.root.join("home"))
            .env("INSTALL_DIR", &self.install_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("installer should start");
        child
            .stdin
            .as_mut()
            .expect("installer stdin")
            .write_all(&installer)
            .expect("pipe installer to sh");
        drop(child.stdin.take());
        child.wait_with_output().expect("installer should finish")
    }
}

fn create_fake_tools(tools: &Path) {
    write_executable(
        &tools.join("uname"),
        r#"#!/bin/sh
case "$1" in
    -s) printf '%s\n' "$FAKE_UNAME_S" ;;
    -m) printf '%s\n' "$FAKE_UNAME_M" ;;
    *) printf 'unexpected uname argument: %s\n' "$1" >&2; exit 2 ;;
esac
"#,
    );
    write_executable(
        &tools.join("curl"),
        r#"#!/bin/sh
set -eu
output=""
url=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) output=$2; shift 2 ;;
        -*) shift ;;
        *) url=$1; shift ;;
    esac
done
case "$url" in
    https://api.github.com/repos/artana-bio/solo-dev/releases/latest)
        source_file="$FAKE_RELEASE_ROOT/release.json"
        ;;
    https://example.invalid/*)
        source_file="$FAKE_RELEASE_ROOT/${url##*/}"
        ;;
    *)
        printf 'unexpected curl URL: %s\n' "$url" >&2
        exit 22
        ;;
esac
if [ -n "$output" ]; then
    cp "$source_file" "$output"
else
    cat "$source_file"
fi
"#,
    );
}

fn create_fake_archive(root: &Path) -> String {
    let asset_name = format!("change-harness-{TAG}-{TARGET}.tar.gz");
    let archive_dir_name = format!("change-harness-{TAG}-{TARGET}");
    let archive_dir = root.join(&archive_dir_name);
    fs::create_dir(&archive_dir).expect("archive directory");
    write_executable(
        &archive_dir.join("change-harness"),
        "#!/bin/sh\nprintf 'change-harness 9.9.9\\n'\n",
    );
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        archive_dir.join("README.md"),
    )
    .expect("README in fake archive");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("LICENSE"),
        archive_dir.join("LICENSE"),
    )
    .expect("LICENSE in fake archive");

    let tar_status = Command::new("tar")
        .args(["czf"])
        .arg(root.join(&asset_name))
        .arg("-C")
        .arg(root)
        .arg(&archive_dir_name)
        .status()
        .expect("tar should run");
    assert!(tar_status.success(), "tar should build the fake release");

    let archive = fs::read(root.join(&asset_name)).expect("fake release archive");
    let checksum = format!("{:x}", Sha256::digest(archive));
    fs::write(
        root.join("SHA256SUMS"),
        format!("{checksum}  {asset_name}\n"),
    )
    .expect("fake checksums");
    asset_name
}

fn write_fake_release_response(root: &Path, asset_name: &str) {
    let response = format!(
        concat!(
            "{{\n",
            "  \"tag_name\": \"{tag}\",\n",
            "  \"assets\": [\n",
            "    {{\"browser_download_url\": \"https://example.invalid/{asset_name}\"}},\n",
            "    {{\"browser_download_url\": \"https://example.invalid/SHA256SUMS\"}}\n",
            "  ]\n",
            "}}\n"
        ),
        tag = TAG,
        asset_name = asset_name,
    );
    fs::write(root.join("release.json"), response).expect("fake release response");
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable fixture");
    let mut permissions = fs::metadata(path).expect("fixture metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("fixture permissions");
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn release_workflow_parses_and_names_every_supported_target() {
    let workflow = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/release.yml"),
    )
    .expect("release workflow");
    let parsed: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&workflow).expect("release workflow should be valid YAML");

    assert_eq!(parsed["on"]["push"]["tags"][0].as_str(), Some("v*"));
    assert_eq!(parsed["permissions"]["contents"].as_str(), Some("write"));
    let targets = parsed["jobs"]["build"]["strategy"]["matrix"]["include"]
        .as_sequence()
        .expect("build matrix")
        .iter()
        .map(|entry| entry["target"].as_str().expect("matrix target"))
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
        ]
    );
}

#[test]
fn installer_selects_verifies_and_replaces_the_native_archive() {
    let release = FakeRelease::new();
    fs::create_dir(&release.install_dir).expect("install directory");
    fs::write(release.install_dir.join("change-harness"), "old binary\n").expect("existing binary");

    let output = release.run("Darwin", "arm64");
    let stdout = text(&output.stdout);
    println!("installer success stdout:\n{stdout}");

    assert!(
        output.status.success(),
        "installer failed: {}",
        text(&output.stderr)
    );
    assert!(stdout.contains("Replacing existing change-harness"));
    assert!(stdout.contains("Installed change-harness 9.9.9"));
    let installed = release.install_dir.join("change-harness");
    assert!(installed.is_file());
    let version = Command::new(installed)
        .arg("--version")
        .output()
        .expect("installed fixture binary should run");
    assert_eq!(text(&version.stdout), "change-harness 9.9.9\n");
}

#[test]
fn installer_checksum_mismatch_leaves_no_installation() {
    let release = FakeRelease::new();
    fs::write(
        release.root.join("SHA256SUMS"),
        format!("{}  {}\n", "0".repeat(64), release.asset_name),
    )
    .expect("corrupt checksums");

    let output = release.run("Darwin", "arm64");
    let stderr = text(&output.stderr);
    println!("checksum mismatch stderr:\n{stderr}");

    assert!(!output.status.success());
    assert!(stderr.contains("checksum mismatch"));
    assert!(!release.install_dir.exists());
}

#[test]
fn installer_rejects_an_unsupported_operating_system() {
    let release = FakeRelease::new();
    let output = release.run("Plan9", "x86_64");
    let stderr = text(&output.stderr);
    println!("unsupported OS stderr:\n{stderr}");

    assert!(!output.status.success());
    assert!(stderr.contains("unsupported operating system 'Plan9'"));
    assert!(!release.install_dir.exists());
}

#[test]
fn installer_rejects_an_unsupported_architecture() {
    let release = FakeRelease::new();
    let output = release.run("Linux", "sparc64");
    let stderr = text(&output.stderr);
    println!("unsupported architecture stderr:\n{stderr}");

    assert!(!output.status.success());
    assert!(stderr.contains("unsupported architecture 'sparc64'"));
    assert!(!release.install_dir.exists());
}
