use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const MAX_FILES: usize = 20_000;
const MAX_BYTES: u64 = 256 * 1024 * 1024;

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn safe_relative(relative: &str) -> Result<PathBuf> {
    let path = PathBuf::from(relative);
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        || relative.contains(':')
    {
        bail!("path must be relative, without parent traversal or alternate data streams");
    }
    Ok(path)
}

fn ordinary(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).context("inspect capture source")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("reparse points are not supported in managed artifacts");
        }
    }
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        bail!("only ordinary files and directories may be captured");
    }
    Ok(metadata)
}

pub(super) fn resolve(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = safe_relative(relative)?;
    let root = root.canonicalize().context("resolve workspace root")?;
    let mut path = root.clone();
    for part in relative.components() {
        path.push(part);
        ordinary(&path)?;
    }
    let canonical = path.canonicalize().context("resolve workspace path")?;
    if !canonical.starts_with(&root) {
        bail!("path escapes workspace");
    }
    Ok(canonical)
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    relative: &Path,
    entries: &mut Vec<Value>,
    bytes: &mut u64,
) -> Result<()> {
    let metadata = ordinary(source)?;
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        if !relative.as_os_str().is_empty() {
            if entries.len() >= MAX_FILES {
                bail!("artifact exceeds entry limit");
            }
            entries.push(
                json!({"path":relative.to_string_lossy().replace('\\', "/"),"kind":"Directory"}),
            );
        }
        let mut children = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            if child.file_name().eq_ignore_ascii_case(".git") {
                continue;
            }
            copy_tree(
                &child.path(),
                &destination.join(child.file_name()),
                &relative.join(child.file_name()),
                entries,
                bytes,
            )?;
        }
    } else {
        *bytes = bytes
            .checked_add(metadata.len())
            .context("artifact size overflow")?;
        if *bytes > MAX_BYTES || entries.len() >= MAX_FILES {
            bail!("artifact exceeds capture limits");
        }
        let content = fs::read(source)?;
        if content.len() as u64 != metadata.len() {
            bail!("source changed during capture");
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, &content)?;
        entries.push(json!({"path":relative.to_string_lossy().replace('\\', "/"),"size":content.len(),"digest":digest(&content)}));
    }
    Ok(())
}

fn readonly_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            readonly_tree(&entry.path())?;
        } else {
            let mut permissions = entry.metadata()?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(entry.path(), permissions)?;
        }
    }
    Ok(())
}

fn publication_contention(error: &std::io::Error) -> bool {
    // Windows can report ACCESS_DENIED rather than SHARING_VIOLATION when a
    // descendant file is open without FILE_SHARE_DELETE during a directory move.
    // Persistent ACL failures share that code but still exhaust this small budget.
    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

fn publish_capture(staging: &Path, published: &Path, mut wait: impl FnMut(Duration)) -> Result<()> {
    const DELAYS_MS: [u64; 5] = [20, 40, 80, 160, 320];
    let mut attempt = 0;
    loop {
        match fs::symlink_metadata(published) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect artifact publication destination"),
            Ok(_) => bail!(
                "artifact publication destination already exists: {}",
                published.display()
            ),
        }
        match fs::rename(staging, published) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let Some(delay) = DELAYS_MS
                    .get(attempt)
                    .filter(|_| publication_contention(&error))
                else {
                    return Err(error).with_context(|| {
                        format!(
                            "publish immutable capture from {} to {} after {} attempt(s)",
                            staging.display(),
                            published.display(),
                            attempt + 1
                        )
                    });
                };
                tracing::debug!(target:"agent_center", attempt = attempt + 1,
                    retry_delay_ms = delay, os_error = error.raw_os_error(),
                    "artifact publication is temporarily denied; retaining sealed staging");
                // capture runs on the existing spawn_blocking boundary. Retry only
                // the atomic rename, never recapture mutable workspace contents.
                wait(Duration::from_millis(*delay));
                attempt += 1;
            }
        }
    }
}

fn remove_staging(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        let metadata = ordinary(&path)?;
        if metadata.is_dir() {
            remove_staging(&path)?;
        } else {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(&path, permissions)?;
            fs::remove_file(&path)?;
        }
    }
    fs::remove_dir(path)?;
    Ok(())
}

pub(in crate::agent_center) fn capture(
    root: &Path,
    workspace: &Path,
    source: &Value,
) -> Result<Value> {
    let kind = source["kind"]
        .as_str()
        .context("capture source kind is required")?;
    if !matches!(kind, "File" | "Tree") {
        bail!("GitCommit capture must first resolve its immutable repository tree");
    }
    if source.get("commitId").is_some() {
        bail!("File/Tree forbids commitId");
    }
    let relative = source["relativePath"]
        .as_str()
        .context("relativePath is required")?;
    let path = resolve(workspace, relative)?;
    let metadata = ordinary(&path)?;
    if (kind == "File") != metadata.is_file() {
        bail!("source kind does not match filesystem object");
    }
    let artifact_id = Uuid::new_v4().to_string();
    let file_name = path
        .file_name()
        .context("capture source has no filename")?
        .to_owned();
    let staging = root.join("staging").join(&artifact_id);
    let published = root.join("artifacts").join(&artifact_id);
    fs::create_dir_all(staging.join("content"))?;
    let outcome = (|| -> Result<Value> {
        let mut entries = Vec::new();
        let mut bytes = 0;
        if kind == "File" {
            copy_tree(
                &path,
                &staging.join("content").join(&file_name),
                Path::new(&file_name),
                &mut entries,
                &mut bytes,
            )?;
        } else {
            copy_tree(
                &path,
                &staging.join("content"),
                Path::new(""),
                &mut entries,
                &mut bytes,
            )?;
        }
        entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        // This closed manifest contains only strings and integers; sorted serde_json
        // object keys and UTF-8 paths have the RFC 8785 representation for these values.
        let manifest = json!({"kind":kind,"entries":entries});
        let encoded = serde_json::to_vec(&manifest)?;
        fs::write(staging.join("manifest.json"), &encoded)?;
        readonly_tree(&staging)?;
        fs::create_dir_all(root.join("artifacts"))?;
        publish_capture(&staging, &published, std::thread::sleep)?;
        let locator = if kind == "File" {
            published.join("content").join(&file_name)
        } else {
            published.join("content")
        };
        Ok(json!({
            "artifactId":artifact_id, "digest":digest(&encoded), "kind":kind,
            "manifest":manifest, "localPath":locator,
            "availability":"Ready"
        }))
    })();
    if outcome.is_err() && staging.exists() {
        if let Err(error) = remove_staging(&staging) {
            tracing::warn!(target:"agent_center", %error, staging_path = %staging.display(),
                "failed to remove incomplete capture staging; no artifact was published");
        }
    }
    outcome
}

pub(in crate::agent_center) fn verify(record: &Value) -> Result<PathBuf> {
    let manifest = &record["manifest"];
    if digest(&serde_json::to_vec(manifest)?) != record["digest"] {
        bail!("artifact manifest digest mismatch");
    }
    let kind = record["artifactKind"]
        .as_str()
        .or_else(|| record["kind"].as_str())
        .context("artifact kind missing")?;
    let manifest_kind = manifest["kind"].as_str().context("manifest kind missing")?;
    if !matches!(
        (kind, manifest_kind),
        ("File", "File") | ("Tree" | "GitCommit", "Tree")
    ) || manifest.as_object().is_none_or(|object| object.len() != 2)
    {
        bail!("artifact kind does not match its capture manifest");
    }
    let locator = PathBuf::from(
        record["localPath"]
            .as_str()
            .context("artifact has no local read locator")?,
    );
    if !locator.is_absolute() {
        bail!("artifact read locator must be absolute");
    }
    // Check before canonicalizing: canonicalize alone would hide substituted
    // junctions/symlinks in the content root or one of its ancestors.
    for ancestor in locator.ancestors() {
        ordinary(ancestor)?;
    }
    let metadata = ordinary(&locator)?;
    if (manifest_kind == "File") != metadata.is_file() {
        bail!("artifact locator has a different filesystem kind");
    }
    let root = if manifest_kind == "File" {
        locator
            .parent()
            .context("file artifact has no parent")?
            .to_owned()
    } else {
        locator.clone()
    };
    if root.file_name().is_none_or(|name| name != "content") {
        bail!("artifact locator is not a managed capture content root");
    }
    let manifest_path = root
        .parent()
        .context("artifact capture root missing")?
        .join("manifest.json");
    let metadata = ordinary(&manifest_path)?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        bail!("artifact on-disk manifest is unavailable or oversized");
    }
    let on_disk: Value =
        serde_json::from_slice(&fs::read(&manifest_path)?).context("read captured manifest")?;
    if on_disk != *manifest {
        bail!("artifact on-disk manifest differs from recorded capture");
    }
    verify_content(
        manifest,
        &root,
        (manifest_kind == "File").then_some(locator.as_path()),
    )?;
    Ok(root)
}

fn verify_content(manifest: &Value, root: &Path, file_locator: Option<&Path>) -> Result<()> {
    let entries = manifest["entries"]
        .as_array()
        .context("artifact manifest entries missing")?;
    if entries.len() > MAX_FILES || (manifest["kind"] == "File" && entries.len() != 1) {
        bail!("artifact manifest has an invalid entry count");
    }
    let mut expected = BTreeSet::new();
    let mut total_bytes = 0_u64;
    for entry in entries {
        let relative = entry["path"].as_str().context("manifest path missing")?;
        let normalized = safe_relative(relative)?;
        if normalized.as_os_str().is_empty()
            || normalized
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || normalized.to_string_lossy().replace('\\', "/") != relative
            || !expected.insert(relative.to_owned())
        {
            bail!("artifact manifest contains an invalid or duplicate path");
        }
        let path = resolve(root, relative)?;
        let metadata = ordinary(&path)?;
        if file_locator
            .is_some_and(|locator| root.join(&normalized) != locator || !metadata.is_file())
        {
            bail!("file artifact locator does not match its manifest entry");
        }
        if entry["kind"] == "Directory" {
            if !metadata.is_dir() || entry.as_object().is_none_or(|object| object.len() != 2) {
                bail!("artifact directory is unavailable");
            }
            continue;
        }
        if !metadata.is_file()
            || entry.as_object().is_none_or(|object| object.len() != 3)
            || Some(metadata.len()) != entry["size"].as_u64()
        {
            bail!("artifact file metadata differs from captured manifest");
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .context("artifact size overflow")?;
        if total_bytes > MAX_BYTES {
            bail!("artifact exceeds capture size limit");
        }
        let content = fs::read(path)?;
        if digest(&content) != entry["digest"]
            || Some(content.len() as u64) != entry["size"].as_u64()
        {
            bail!("artifact captured content digest mismatch");
        }
    }
    let mut actual = BTreeSet::new();
    collect_paths(root, root, &mut actual)?;
    if actual != expected {
        bail!("artifact content tree differs from captured manifest");
    }
    Ok(())
}

fn collect_paths(root: &Path, directory: &Path, paths: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let metadata = ordinary(&path)?;
        if paths.len() >= MAX_FILES {
            bail!("artifact exceeds capture entry limit");
        }
        paths.insert(
            path.strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/"),
        );
        if metadata.is_dir() {
            collect_paths(root, &path, paths)?;
        }
    }
    Ok(())
}

pub(super) fn materialize(record: &Value, destination: &Path) -> Result<()> {
    let source = verify(record)?;
    if destination.exists() {
        bail!("input materialization destination already exists");
    }

    fs::create_dir_all(destination)?;
    for entry in record["manifest"]["entries"]
        .as_array()
        .context("manifest entries missing")?
    {
        let relative = entry["path"].as_str().context("manifest path missing")?;
        let from = resolve(&source, relative)?;
        let to = destination.join(safe_relative(relative)?);
        if entry["kind"] == "Directory" {
            fs::create_dir_all(to)?;
            continue;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to)?;
    }
    verify_content(&record["manifest"], destination, None)?;
    readonly_tree(destination)
}

pub(super) fn verify_materialized(record: &Value, destination: &Path) -> Result<()> {
    verify(record)?;
    for ancestor in destination.ancestors() {
        ordinary(ancestor)?;
    }
    verify_content(&record["manifest"], destination, None)
}

pub(super) fn check_working_copy(
    dispatch: &Value,
    records: &[(Value, Value)],
    destination: &Path,
) -> Result<()> {
    if records.is_empty() {
        bail!("command gate requires pinned inputs or submitted artifacts");
    }
    if destination.exists() {
        bail!("command check destination already exists");
    }
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut submitted_snapshot = false;
    for (reference, record) in records {
        if record["digest"] != reference["artifact"]["digest"] {
            bail!("pinned input digest differs from captured artifact");
        }
        if reference["sourceResultId"].is_string()
            && reference["sourceResultId"] == dispatch["subjectResultId"]
        {
            submitted_snapshot |= record["manifest"]["kind"] == "Tree"
                && (record["artifactKind"] == "GitCommit"
                    || dispatch["outputs"].as_array().is_some_and(|contracts| {
                        contracts.iter().any(|contract| {
                            contract["slot"] == reference["slot"]
                                && matches!(
                                    contract["kind"].as_str(),
                                    Some("Code" | "Tree" | "GitCommit")
                                )
                        })
                    }));
            outputs.push(record);
        } else {
            inputs.push(record);
        }
    }
    // Superseded snapshots must not resurrect deleted code. Independently pinned
    // File inputs, such as check scripts and data, remain part of the check.
    if submitted_snapshot {
        inputs.retain(|record| record["manifest"]["kind"] != "Tree");
    }
    let mut combined: BTreeMap<String, (PathBuf, Value)> = BTreeMap::new();
    for (label, records) in [("pinned inputs", inputs), ("submitted outputs", outputs)] {
        let mut layer: BTreeMap<String, (PathBuf, Value)> = BTreeMap::new();
        for record in records {
            let source =
                verify(record).with_context(|| format!("verify {label} for command check"))?;
            for entry in record["manifest"]["entries"]
                .as_array()
                .context("manifest entries missing")?
            {
                let relative = entry["path"].as_str().context("manifest path missing")?;
                let key = if cfg!(windows) {
                    relative.to_lowercase()
                } else {
                    relative.to_owned()
                };
                if let Some((_, previous)) = layer.get(&key) {
                    if previous["kind"] != entry["kind"]
                        || previous["digest"] != entry["digest"]
                        || previous["size"] != entry["size"]
                    {
                        bail!("conflicting command-check path {relative:?} in {label}; capture one consistent Tree instead of ambiguous overlapping artifacts");
                    }
                } else {
                    layer.insert(key, (source.clone(), entry.clone()));
                }
            }
        }
        for (key, entry) in layer {
            if combined
                .get(&key)
                .is_some_and(|(_, previous)| previous["kind"] != entry.1["kind"])
            {
                bail!("command-check path {:?} changes between a file and directory; submit a complete Tree snapshot", entry.1["path"]);
            }
            // Only the submitted-result layer can supersede pinned input bytes.
            combined.insert(key, entry);
        }
    }
    let total_bytes = combined.values().try_fold(0_u64, |total, (_, entry)| {
        total
            .checked_add(entry["size"].as_u64().unwrap_or(0))
            .context("command check size overflow")
    })?;
    if combined.len() > MAX_FILES || total_bytes > MAX_BYTES {
        bail!("combined command-check inputs exceed capture limits");
    }
    fs::create_dir_all(destination).context("create immutable-input command check directory")?;
    for (source, entry) in combined.values() {
        let relative = entry["path"].as_str().context("manifest path missing")?;
        let target = destination.join(safe_relative(relative)?);
        if entry["kind"] == "Directory" {
            fs::create_dir_all(&target)?;
        } else {
            let content = fs::read(resolve(source, relative)?)?;
            if digest(&content) != entry["digest"]
                || Some(content.len() as u64) != entry["size"].as_u64()
            {
                bail!("pinned input changed while preparing command-check path {relative:?}");
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?
                .write_all(&content)?;
        }
    }
    let manifest = json!({"kind":"Tree","entries":combined.values().map(|(_, entry)| entry).collect::<Vec<_>>()});
    verify_content(&manifest, destination, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PublicationRoot(PathBuf);

    impl Drop for PublicationRoot {
        fn drop(&mut self) {
            if let Err(error) = remove_staging(&self.0) {
                eprintln!(
                    "Could not clean publication fixture {}: {error:#}",
                    self.0.display()
                );
            }
        }
    }

    fn check_fixture() -> (PublicationRoot, PathBuf) {
        let root = PublicationRoot(
            std::env::current_dir()
                .unwrap()
                .join("target")
                .join(format!("center-check-{}", Uuid::new_v4())),
        );
        let workspace = root.0.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        (root, workspace)
    }

    fn check_input(
        root: &Path,
        workspace: &Path,
        source: Value,
        result: &str,
        slot: &str,
    ) -> (Value, Value) {
        let artifact = capture(root, workspace, &source).unwrap();
        let reference = json!({
            "slot":slot,"sourceResultId":result,"sourceGateIds":[],
            "artifact":{"artifactId":artifact["artifactId"],"digest":artifact["digest"]}
        });
        (reference, artifact)
    }

    #[cfg(windows)]
    async fn check_script(directory: &Path, script: &str) -> super::super::process::CommandOutcome {
        let recipe = serde_json::from_value(json!({
            "executable":"powershell.exe",
            "args":["-NoLogo","-NoProfile","-NonInteractive","-Command",script],
            "cwdRelative":".","timeoutSeconds":30,"environmentRef":"local-default","evidenceParserId":"process-exit-v1"
        })).unwrap();
        super::super::process::run(
            &recipe,
            directory,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn file_only_command_checks_use_captured_bytes_not_mutable_workspace() {
        let (root, workspace) = check_fixture();
        fs::create_dir(workspace.join("docs")).unwrap();
        fs::write(
            workspace.join("docs").join("report.txt"),
            b"verified report",
        )
        .unwrap();
        fs::write(workspace.join("evidence.json"), br#"{"count":7}"#).unwrap();
        let report = check_input(
            &root.0,
            &workspace,
            json!({"kind":"File","relativePath":"docs/report.txt"}),
            "subject",
            "report",
        );
        let evidence = check_input(
            &root.0,
            &workspace,
            json!({"kind":"File","relativePath":"evidence.json"}),
            "subject",
            "evidence",
        );
        fs::write(
            workspace.join("docs").join("report.txt"),
            b"mutable changed report",
        )
        .unwrap();
        fs::write(workspace.join("only-in-workspace.txt"), b"not submitted").unwrap();
        let destination = root.0.join("check");
        check_working_copy(
            &json!({"subjectResultId":"subject"}),
            &[report.clone(), evidence],
            &destination,
        )
        .unwrap();
        let outcome = check_script(
            &destination,
            r#"
            if ((Get-Content -Raw .\report.txt) -cne 'verified report') { exit 11 }
            if ((Get-Content -Raw .\evidence.json | ConvertFrom-Json).count -ne 7) { exit 12 }
            if (Test-Path .\only-in-workspace.txt) { exit 13 }
            [IO.File]::WriteAllText((Join-Path (Get-Location) 'report.txt'), 'check-local write')
            [Console]::Out.Write('fixed-file-check-passed')
        "#,
        )
        .await;
        assert_eq!(outcome.disposition(), "Passed", "{outcome:?}");
        assert_eq!(outcome.stdout, "fixed-file-check-passed");
        assert_eq!(
            fs::read(verify(&report.1).unwrap().join("report.txt")).unwrap(),
            b"verified report"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn report_checks_include_exact_dependency_snapshot_and_current_output_bytes() {
        let (root, workspace) = check_fixture();
        fs::write(workspace.join("value.txt"), b"7").unwrap();
        fs::write(workspace.join("report.txt"), b"old report").unwrap();
        fs::write(
            workspace.join("check.ps1"),
            r#"
            if ((Get-Content -Raw .\value.txt) -ne '7') { exit 21 }
            if ((Get-Content -Raw .\report.txt) -cne 'new report') { exit 22 }
            [Console]::Out.Write('fixed-dependency-check-passed')
        "#,
        )
        .unwrap();
        let dependency = check_input(
            &root.0,
            &workspace,
            json!({"kind":"Tree","relativePath":"."}),
            "upstream",
            "code",
        );
        fs::write(workspace.join("report.txt"), b"new report").unwrap();
        let report = check_input(
            &root.0,
            &workspace,
            json!({"kind":"File","relativePath":"report.txt"}),
            "subject",
            "report",
        );
        fs::write(workspace.join("value.txt"), b"999").unwrap();
        fs::write(workspace.join("check.ps1"), "exit 23").unwrap();
        let destination = root.0.join("check");
        check_working_copy(
            &json!({"subjectResultId":"subject"}),
            &[report, dependency],
            &destination,
        )
        .unwrap();
        let outcome = check_script(&destination, "& .\\check.ps1").await;
        assert_eq!(outcome.disposition(), "Passed", "{outcome:?}");
        assert_eq!(outcome.stdout, "fixed-dependency-check-passed");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn complete_submitted_snapshot_does_not_resurrect_deleted_dependency_files() {
        let (root, workspace) = check_fixture();
        fs::write(workspace.join("deleted.txt"), b"old input").unwrap();
        let dependency = check_input(
            &root.0,
            &workspace,
            json!({"kind":"Tree","relativePath":"."}),
            "upstream",
            "prior-code",
        );
        fs::remove_file(workspace.join("deleted.txt")).unwrap();
        fs::write(workspace.join("value.txt"), b"new code").unwrap();
        let output = check_input(
            &root.0,
            &workspace,
            json!({"kind":"Tree","relativePath":"."}),
            "subject",
            "code",
        );
        let support = root.0.join("support");
        fs::create_dir(&support).unwrap();
        fs::write(
            support.join("check.ps1"),
            r#"
            if (Test-Path .\deleted.txt) { exit 31 }
            if ((Get-Content -Raw .\value.txt) -cne 'new code') { exit 32 }
            [Console]::Out.Write('deleted-file-stays-deleted')
        "#,
        )
        .unwrap();
        let script = check_input(
            &root.0,
            &support,
            json!({"kind":"File","relativePath":"check.ps1"}),
            "test-input",
            "check-script",
        );
        let destination = root.0.join("check");
        check_working_copy(
            &json!({"subjectResultId":"subject","outputs":[{"slot":"code","kind":"Code","required":true}]}),
            &[dependency, output, script],
            &destination,
        ).unwrap();
        let outcome = check_script(&destination, "& .\\check.ps1").await;
        assert_eq!(outcome.disposition(), "Passed", "{outcome:?}");
        assert_eq!(outcome.stdout, "deleted-file-stays-deleted");
    }

    #[test]
    fn ambiguous_check_inputs_fail_before_launch_and_duplicate_materializations_are_verified() {
        let (root, workspace) = check_fixture();
        fs::write(workspace.join("report.txt"), b"first").unwrap();
        let first = check_input(
            &root.0,
            &workspace,
            json!({"kind":"File","relativePath":"report.txt"}),
            "subject",
            "first",
        );
        fs::write(workspace.join("report.txt"), b"second").unwrap();
        let second = check_input(
            &root.0,
            &workspace,
            json!({"kind":"File","relativePath":"report.txt"}),
            "subject",
            "second",
        );
        let destination = root.0.join("check");
        let error = check_working_copy(
            &json!({"subjectResultId":"subject"}),
            &[first.clone(), second],
            &destination,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("conflicting command-check path \"report.txt\""),
            "{error:#}"
        );
        assert!(!destination.exists());
        check_working_copy(
            &json!({"subjectResultId":"subject"}),
            &[first.clone(), first.clone()],
            &destination,
        )
        .unwrap();
        assert_eq!(fs::read(destination.join("report.txt")).unwrap(), b"first");
        let materialized = root.0.join("materialized");
        materialize(&first.1, &materialized).unwrap();
        verify_materialized(&first.1, &materialized).unwrap();
        fs::write(materialized.join("unrecorded.txt"), b"not captured").unwrap();
        assert!(verify_materialized(&first.1, &materialized).is_err());
        let mut tampered = first;
        tampered.1["manifest"]["entries"][0]["digest"] = json!("sha256:changed");
        let error = check_working_copy(
            &json!({"subjectResultId":"subject"}),
            &[tampered],
            &root.0.join("tampered"),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("artifact manifest digest mismatch"),
            "{error:#}"
        );
    }

    fn publication_fixture() -> (PublicationRoot, PathBuf, PathBuf, Value) {
        let root = PublicationRoot(
            std::env::current_dir()
                .unwrap()
                .join("target")
                .join(format!("center-publication-{}", Uuid::new_v4())),
        );
        let workspace = root.0.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("report.txt"), b"fixed publication bytes").unwrap();
        let artifact = capture(
            &root.0,
            &workspace,
            &json!({"kind":"File","relativePath":"report.txt"}),
        )
        .unwrap();
        let published = root
            .0
            .join("artifacts")
            .join(artifact["artifactId"].as_str().unwrap());
        let staging = root
            .0
            .join("staging")
            .join(artifact["artifactId"].as_str().unwrap());
        publish_capture(&published, &staging, std::thread::sleep).unwrap();
        (root, staging, published, artifact)
    }

    #[cfg(windows)]
    #[test]
    fn publication_recovers_after_real_windows_delete_sharing_lock_is_released() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_root, staging, published, artifact) = publication_fixture();
        let captured = staging.join("content").join("report.txt");
        let mut lock = Some(
            fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(staging.join("manifest.json"))
                .unwrap(),
        );
        let error = fs::rename(&staging, &published).unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(5),
            "directory move with locked descendant: {error}"
        );
        let mut waits = 0;
        publish_capture(&staging, &published, |delay| {
            waits += 1;
            assert_eq!(delay, Duration::from_millis(20));
            assert!(!published.exists());
            assert_eq!(fs::read(&captured).unwrap(), b"fixed publication bytes");
            assert!(fs::metadata(&captured).unwrap().permissions().readonly());
            drop(lock.take());
        })
        .unwrap();
        assert_eq!(waits, 1);
        assert!(!staging.exists());
        let content = verify(&artifact).unwrap();
        assert!(fs::metadata(content.join("report.txt"))
            .unwrap()
            .permissions()
            .readonly());
        assert!(fs::metadata(published.join("manifest.json"))
            .unwrap()
            .permissions()
            .readonly());
    }

    #[cfg(windows)]
    #[test]
    fn publication_permanent_windows_sharing_lock_exhausts_only_bounded_rename_budget() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_root, staging, published, artifact) = publication_fixture();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(staging.join("manifest.json"))
            .unwrap();
        let mut delays = Vec::new();
        let error = publish_capture(&staging, &published, |delay| delays.push(delay)).unwrap_err();
        assert_eq!(delays, [20, 40, 80, 160, 320].map(Duration::from_millis));
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(5)
        );
        assert!(error.to_string().contains("after 6 attempt(s)"));
        assert!(error.to_string().contains(&staging.display().to_string()));
        assert!(error.to_string().contains(&published.display().to_string()));
        assert!(!published.exists());
        assert!(verify(&artifact).is_err());
        assert!(fs::metadata(staging.join("manifest.json"))
            .unwrap()
            .permissions()
            .readonly());
        drop(lock);
        remove_staging(&staging).unwrap();
        assert!(!staging.exists());
    }

    #[test]
    fn publication_never_replaces_destination_or_retries_noncontention_errors() {
        let (_root, staging, published, _) = publication_fixture();
        fs::create_dir_all(&published).unwrap();
        fs::write(published.join("existing.txt"), b"do not overwrite").unwrap();
        let error = publish_capture(&staging, &published, |_| {
            panic!("destination collision must not retry")
        })
        .unwrap_err();
        assert!(error.to_string().contains("destination already exists"));
        assert_eq!(
            fs::read(published.join("existing.txt")).unwrap(),
            b"do not overwrite"
        );
        assert!(staging.join("manifest.json").is_file());
        let missing_source = staging.with_file_name("missing-source");
        let absent_destination = published.with_file_name("absent-destination");
        let error = publish_capture(&missing_source, &absent_destination, |_| {
            panic!("missing source must not retry")
        })
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(!absent_destination.exists());
        assert!(!publication_contention(&std::io::Error::from_raw_os_error(
            2
        )));
    }

    #[test]
    fn rejects_traversal_and_streams() {
        for path in ["..\\secret", "C:\\secret", "a:secret", "\\\\server\\share"] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        assert!(safe_relative("report\\answer.md").is_ok());
    }

    #[test]
    fn capture_digest_is_fixed_and_detects_mutation() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("center-artifact-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("report.txt"), "original").unwrap();
        let artifact = capture(
            &root,
            &workspace,
            &json!({"kind":"File","relativePath":"report.txt"}),
        )
        .unwrap();
        fs::write(workspace.join("report.txt"), "changed").unwrap();
        let content = verify(&artifact).unwrap();
        assert_eq!(
            fs::read_to_string(content.join("report.txt")).unwrap(),
            "original"
        );
        assert!(artifact["digest"].as_str().unwrap().starts_with("sha256:"));
        let mut corrupt = artifact;
        corrupt["manifest"]["entries"][0]["digest"] = json!("sha256:bad");
        assert!(verify(&corrupt).is_err());
        // Windows read-only capture files require an explicit test cleanup attribute reset.
        fn clean(path: &Path) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    clean(&entry.path());
                } else {
                    let mut permissions = entry.metadata().unwrap().permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    permissions.set_readonly(false);
                    fs::set_permissions(entry.path(), permissions).unwrap();
                }
            }
        }
        clean(&root);
        fs::remove_dir_all(root).unwrap();
    }
}
