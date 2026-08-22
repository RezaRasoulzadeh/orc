use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const RUNTIME_ARTIFACTS: [&str; 3] = [".orc/orc.db", ".orc/orc.db-wal", ".orc/orc.db-shm"];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeChanges {
    pub files: Vec<ChangedFile>,
    pub stat: String,
    pub diff: String,
}

pub fn is_runtime_artifact(path: &str) -> bool {
    RUNTIME_ARTIFACTS.contains(&path)
        || path == ".orc/worktrees"
        || path.starts_with(".orc/worktrees/")
}

fn git_output(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_output_owned(dir: &Path, args: &[String]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn changed_files(worktree: impl AsRef<Path>) -> Result<Vec<ChangedFile>> {
    let worktree = worktree.as_ref();
    let output = git_output(worktree, &["status", "--porcelain=v1", "-z"])?;
    let mut files = Vec::new();
    let mut entries = output.split('\0');
    while let Some(entry) = entries.next() {
        if entry.is_empty() {
            continue;
        }
        if entry.len() < 4 {
            continue;
        }
        let status = entry[..2].trim().to_string();
        let path = entry[3..].to_string();
        if status.contains('R') || status.contains('C') {
            let _ = entries.next();
        }
        if !is_runtime_artifact(&path) {
            files.push(ChangedFile { status, path });
        }
    }
    Ok(files)
}

pub fn inspect_worktree(
    worktree: impl AsRef<Path>,
    main_checkout: impl AsRef<Path>,
) -> Result<WorktreeChanges> {
    let worktree = worktree.as_ref();
    let main_head = git_output(main_checkout.as_ref(), &["rev-parse", "HEAD"])?;
    let base = git_output_owned(
        worktree,
        &[
            "merge-base".to_owned(),
            main_head.trim().to_owned(),
            "HEAD".to_owned(),
        ],
    )?;
    let base = base.trim();
    let diff_args = [
        "diff".to_owned(),
        base.to_owned(),
        "--".to_owned(),
        ".".to_owned(),
        ":(exclude).orc/orc.db".to_owned(),
        ":(exclude).orc/orc.db-wal".to_owned(),
        ":(exclude).orc/orc.db-shm".to_owned(),
        ":(exclude).orc/worktrees/**".to_owned(),
    ];
    let mut diff = git_output_owned(worktree, &diff_args)?;
    let status_files = changed_files(worktree)?;
    let names_args = [
        "diff".to_owned(),
        "--name-status".to_owned(),
        "-z".to_owned(),
        base.to_owned(),
        "--".to_owned(),
        ".".to_owned(),
        ":(exclude).orc/orc.db".to_owned(),
        ":(exclude).orc/orc.db-wal".to_owned(),
        ":(exclude).orc/orc.db-shm".to_owned(),
        ":(exclude).orc/worktrees/**".to_owned(),
    ];
    let mut files = parse_name_status(&git_output_owned(worktree, &names_args)?);
    for file in status_files.iter().filter(|file| file.status == "??") {
        let output = Command::new("git")
            .current_dir(worktree)
            .args(["diff", "--no-index", "--", "/dev/null", &file.path])
            .output()
            .context("failed to diff untracked file")?;
        if output.status.code() == Some(1) {
            diff.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        files.push(file.clone());
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    let stat = if files.is_empty() {
        String::new()
    } else {
        let stat_args = [
            "diff".to_owned(),
            "--stat".to_owned(),
            base.to_owned(),
            "--".to_owned(),
            ".".to_owned(),
            ":(exclude).orc/orc.db".to_owned(),
            ":(exclude).orc/orc.db-wal".to_owned(),
            ":(exclude).orc/orc.db-shm".to_owned(),
            ":(exclude).orc/worktrees/**".to_owned(),
        ];
        let mut text = git_output_owned(worktree, &stat_args)?;
        if files.iter().any(|file| file.status == "??") {
            text.push_str(&format!(
                "{} file(s) untracked\n",
                files.iter().filter(|file| file.status == "??").count()
            ));
        }
        text.trim().to_owned()
    };
    Ok(WorktreeChanges { files, stat, diff })
}

fn parse_name_status(output: &str) -> Vec<ChangedFile> {
    let mut entries = output.split('\0');
    let mut files = Vec::new();
    while let (Some(status), Some(first_path)) = (entries.next(), entries.next()) {
        let path = if status.starts_with('R') || status.starts_with('C') {
            entries.next().unwrap_or(first_path)
        } else {
            first_path
        };
        if status.is_empty() || is_runtime_artifact(path) {
            continue;
        }
        files.push(ChangedFile {
            status: status.to_owned(),
            path: path.to_owned(),
        });
    }
    files
}

/// Generates a Git branch name for a task.
/// Format: orc/task/<task-id>
pub fn branch_name_for_task(task_id: &str) -> String {
    format!("orc/task/{}", task_id)
}

/// Generates a worktree path for a task.
/// Format: .orc/worktrees/<task-id>
pub fn worktree_path_for_task(task_id: &str) -> PathBuf {
    PathBuf::from(".orc/worktrees").join(task_id)
}

/// Create a git worktree for a task.
/// This creates both the worktree directory and a new branch.
///
/// # Arguments
/// - `task_id`: The task identifier (e.g., "T-0001")
/// - `git_dir`: The root git repository directory (e.g., ".")
///
/// # Returns
/// A tuple of (branch_name, worktree_path)
pub fn create_worktree(task_id: &str, git_dir: impl AsRef<Path>) -> Result<(String, PathBuf)> {
    let git_dir = git_dir.as_ref();
    let branch = branch_name_for_task(task_id);
    let worktree_path = worktree_path_for_task(task_id);

    // Ensure worktree parent directory exists
    let worktree_parent = git_dir.join(".orc/worktrees");
    std::fs::create_dir_all(&worktree_parent).with_context(|| {
        format!(
            "failed to create worktree parent dir: {}",
            worktree_parent.display()
        )
    })?;

    let absolute_worktree_path = git_dir.join(&worktree_path);

    // Create the worktree with a new branch
    let output = Command::new("git")
        .current_dir(git_dir)
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg(&worktree_path)
        .output()
        .context("failed to execute git worktree add")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed for task {}: {}", task_id, stderr);
    }

    // Check if directory actually exists after git worktree add
    if !absolute_worktree_path.exists() {
        anyhow::bail!(
            "git worktree add succeeded but directory does not exist: {}",
            absolute_worktree_path.display()
        );
    }

    // Create and checkout the task branch
    let output = Command::new("git")
        .current_dir(&absolute_worktree_path)
        .arg("checkout")
        .arg("-b")
        .arg(&branch)
        .output()
        .context("failed to create task branch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = Command::new("git")
            .current_dir(git_dir)
            .arg("worktree")
            .arg("remove")
            .arg(&worktree_path)
            .output();
        anyhow::bail!("git checkout -b failed for task {}: {}", task_id, stderr);
    }

    Ok((branch, worktree_path))
}

/// Ensure a git worktree exists for a task. If it already exists, reuses it.
/// If not, creates the worktree and checks out the task branch.
pub fn ensure_worktree(task_id: &str, git_dir: impl AsRef<Path>) -> Result<(String, PathBuf)> {
    let git_dir = git_dir.as_ref();
    let branch = branch_name_for_task(task_id);
    let worktree_path = worktree_path_for_task(task_id);
    let absolute_worktree_path = git_dir.join(&worktree_path);

    if absolute_worktree_path.exists() {
        let is_worktree = Command::new("git")
            .current_dir(&absolute_worktree_path)
            .arg("rev-parse")
            .arg("--is-inside-work-tree")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if is_worktree {
            let current_branch = git_output(
                &absolute_worktree_path,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
            )?;
            if current_branch.trim() == branch {
                return Ok((branch, worktree_path));
            }
            anyhow::bail!(
                "task worktree {} is checked out at unexpected branch {}",
                absolute_worktree_path.display(),
                current_branch.trim()
            );
        }
    }

    let worktree_parent = git_dir.join(".orc/worktrees");
    std::fs::create_dir_all(&worktree_parent).with_context(|| {
        format!(
            "failed to create worktree parent dir: {}",
            worktree_parent.display()
        )
    })?;

    // Check if the branch already exists in git
    let branch_exists = Command::new("git")
        .current_dir(git_dir)
        .arg("show-ref")
        .arg("--verify")
        .arg(format!("refs/heads/{}", branch))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if branch_exists {
        git_output(git_dir, &["worktree", "prune"])?;
        let mut stale_branch = format!("{}.stale", branch);
        let mut suffix = 1;
        while Command::new("git")
            .current_dir(git_dir)
            .args([
                "show-ref",
                "--verify",
                &format!("refs/heads/{stale_branch}"),
            ])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
        {
            stale_branch = format!("{}.stale-{}", branch, suffix);
            suffix += 1;
        }
        git_output_owned(
            git_dir,
            &[
                "branch".into(),
                "-m".into(),
                branch.clone(),
                stale_branch.clone(),
            ],
        )
        .with_context(|| format!("failed to preserve stale task branch {branch}"))?;
        create_worktree(task_id, git_dir)
    } else {
        create_worktree(task_id, git_dir)
    }
}

/// Validates a unified diff patch against a worktree using `git apply --check`.
/// The working tree is NOT modified.
pub fn validate_patch(worktree_dir: impl AsRef<Path>, patch_content: &str) -> Result<()> {
    let worktree_dir = worktree_dir.as_ref();
    if patch_content.trim().is_empty() {
        anyhow::bail!("malformed patch: patch content is empty");
    }

    let mut child = Command::new("git")
        .current_dir(worktree_dir)
        .arg("apply")
        .arg("--check")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git apply --check")?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(patch_content.as_bytes())
            .context("failed to write patch to git apply stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to wait on git apply --check")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let err_detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "patch check failed".to_string()
        };
        anyhow::bail!("patch validation failed: {}", err_detail);
    }

    Ok(())
}

/// Applies a unified diff patch to a worktree using `git apply`.
pub fn apply_patch(worktree_dir: impl AsRef<Path>, patch_content: &str) -> Result<()> {
    let worktree_dir = worktree_dir.as_ref();
    if patch_content.trim().is_empty() {
        anyhow::bail!("malformed patch: patch content is empty");
    }

    let mut child = Command::new("git")
        .current_dir(worktree_dir)
        .arg("apply")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git apply")?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(patch_content.as_bytes())
            .context("failed to write patch to git apply stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to wait on git apply")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let err_detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "patch apply failed".to_string()
        };
        anyhow::bail!("patch apply failed: {}", err_detail);
    }

    Ok(())
}

/// Get the branch name associated with a task worktree.
pub fn get_branch_name(task_id: &str) -> String {
    branch_name_for_task(task_id)
}

/// Get the worktree path for a task.
pub fn get_worktree_path(task_id: &str) -> PathBuf {
    worktree_path_for_task(task_id)
}

pub fn show_diff(task_id: &str, git_dir: impl AsRef<Path>) -> Result<String> {
    let git_dir = git_dir.as_ref();
    let worktree_path = worktree_path_for_task(task_id);
    let absolute_worktree_path = git_dir.join(&worktree_path);

    if !absolute_worktree_path.exists() {
        anyhow::bail!(
            "worktree does not exist for task {}: {}",
            task_id,
            absolute_worktree_path.display()
        );
    }

    Ok(inspect_worktree(&absolute_worktree_path, git_dir)?.diff)
}

pub fn worktree_has_meaningful_changes(worktree: impl AsRef<Path>) -> Result<bool> {
    Ok(!changed_files(worktree)?.is_empty())
}

pub fn commit_worktree_changes(
    worktree: impl AsRef<Path>,
    task_id: &str,
    title: &str,
) -> Result<bool> {
    let worktree = worktree.as_ref();
    if !worktree_has_meaningful_changes(worktree)? {
        return Ok(false);
    }
    git_output(
        worktree,
        &[
            "add",
            "-A",
            "--",
            ".",
            ":(exclude).orc/orc.db",
            ":(exclude).orc/orc.db-wal",
            ":(exclude).orc/orc.db-shm",
            ":(exclude).orc/worktrees/**",
        ],
    )?;
    let staged = git_output(worktree, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok(false);
    }
    let message = format!("Orc task {task_id}: {title}");
    let output = Command::new("git")
        .current_dir(worktree)
        .arg("commit")
        .arg("-m")
        .arg(message)
        .output()
        .context("failed to commit task changes")?;
    if !output.status.success() {
        anyhow::bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(true)
}

pub fn main_checkout_is_clean(repo: impl AsRef<Path>) -> Result<bool> {
    Ok(changed_files(repo)?.is_empty())
}

pub fn merge_task_branch(repo: impl AsRef<Path>, branch: &str, task_id: &str) -> Result<()> {
    let repo = repo.as_ref();
    if !main_checkout_is_clean(repo)? {
        anyhow::bail!(
            "current project checkout has meaningful uncommitted changes; commit or stash them before accepting"
        );
    }
    let message = format!("Merge Orc task {task_id}");
    let output = Command::new("git")
        .current_dir(repo)
        .args(["merge", "--no-ff", branch, "-m", &message])
        .output()
        .context("failed to merge task branch")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let _ = Command::new("git")
            .current_dir(repo)
            .args(["merge", "--abort"])
            .output();
        anyhow::bail!(
            "task branch conflicts with the current project checkout; merge was aborted: {detail}"
        );
    }
    Ok(())
}

pub fn remove_worktree(repo: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<()> {
    let output = Command::new("git")
        .current_dir(repo.as_ref())
        .args([
            "worktree",
            "remove",
            "--force",
            path.as_ref().to_string_lossy().as_ref(),
        ])
        .output()
        .context("failed to remove task worktree")?;
    if !output.status.success() {
        anyhow::bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_test_repo(repo_path: &Path) {
        Command::new("git")
            .current_dir(repo_path)
            .arg("init")
            .arg(".")
            .output()
            .expect("init repo");

        Command::new("git")
            .current_dir(repo_path)
            .arg("config")
            .arg("user.email")
            .arg("test@example.com")
            .output()
            .expect("config email");

        Command::new("git")
            .current_dir(repo_path)
            .arg("config")
            .arg("user.name")
            .arg("Test User")
            .output()
            .expect("config name");

        let file_path = repo_path.join("README.md");
        std::fs::write(&file_path, "test\n").expect("write file");
        Command::new("git")
            .current_dir(repo_path)
            .arg("add")
            .arg(".")
            .output()
            .expect("git add");
        Command::new("git")
            .current_dir(repo_path)
            .arg("commit")
            .arg("-m")
            .arg("initial")
            .output()
            .expect("git commit");
    }

    #[test]
    fn test_branch_name_generation() {
        assert_eq!(branch_name_for_task("T-0001"), "orc/task/T-0001");
        assert_eq!(branch_name_for_task("T-9999"), "orc/task/T-9999");
    }

    #[test]
    fn test_worktree_path_generation() {
        assert_eq!(
            worktree_path_for_task("T-0001"),
            PathBuf::from(".orc/worktrees/T-0001")
        );
        assert_eq!(
            worktree_path_for_task("T-9999"),
            PathBuf::from(".orc/worktrees/T-9999")
        );
    }

    #[test]
    fn test_worktree_creation_and_branch() {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let repo_path = tmpdir.path();
        init_test_repo(repo_path);

        let (branch, worktree_path) =
            create_worktree("T-0001", repo_path).expect("create worktree");

        assert_eq!(branch, "orc/task/T-0001");
        assert_eq!(worktree_path, PathBuf::from(".orc/worktrees/T-0001"));

        let absolute_worktree_path = repo_path.join(&worktree_path);
        assert!(
            absolute_worktree_path.exists(),
            "worktree path should exist"
        );

        let output = Command::new("git")
            .current_dir(&absolute_worktree_path)
            .arg("rev-parse")
            .arg("--abbrev-ref")
            .arg("HEAD")
            .output()
            .expect("get branch");
        let current_branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(current_branch, "orc/task/T-0001");
    }

    #[test]
    fn test_ensure_worktree_reusable() {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let repo_path = tmpdir.path();
        init_test_repo(repo_path);

        let (branch1, path1) = ensure_worktree("T-0001", repo_path).expect("ensure first");
        let (branch2, path2) = ensure_worktree("T-0001", repo_path).expect("ensure second");

        assert_eq!(branch1, branch2);
        assert_eq!(path1, path2);
        assert!(repo_path.join(&path1).exists());
    }

    #[test]
    fn test_validate_and_apply_patch() {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let repo_path = tmpdir.path();
        init_test_repo(repo_path);

        let (_branch, worktree_path) = ensure_worktree("T-0001", repo_path).expect("ensure");
        let worktree_dir = repo_path.join(worktree_path);

        let valid_patch = "diff --git a/newfile.txt b/newfile.txt
new file mode 100644
--- /dev/null
+++ b/newfile.txt
@@ -0,0 +1 @@
+hello world
";

        // Check validation passes
        assert!(validate_patch(&worktree_dir, valid_patch).is_ok());
        // Validation must not create the file
        assert!(!worktree_dir.join("newfile.txt").exists());

        // Apply patch
        assert!(apply_patch(&worktree_dir, valid_patch).is_ok());
        assert!(worktree_dir.join("newfile.txt").exists());
        assert_eq!(
            std::fs::read_to_string(worktree_dir.join("newfile.txt")).unwrap(),
            "hello world\n"
        );
        // Main checkout remains untouched
        assert!(!repo_path.join("newfile.txt").exists());
    }

    #[test]
    fn test_malformed_and_conflicting_patch_validation_fails() {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let repo_path = tmpdir.path();
        init_test_repo(repo_path);

        let (_branch, worktree_path) = ensure_worktree("T-0001", repo_path).expect("ensure");
        let worktree_dir = repo_path.join(worktree_path);

        // Malformed patch
        assert!(validate_patch(&worktree_dir, "not a real patch").is_err());
        assert!(validate_patch(&worktree_dir, "").is_err());

        // Conflicting patch (expects different content in README.md)
        let conflict_patch = "diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-nonexistent line
+updated
";
        assert!(validate_patch(&worktree_dir, conflict_patch).is_err());
    }

    #[test]
    fn test_get_branch_name() {
        assert_eq!(get_branch_name("T-0001"), "orc/task/T-0001");
    }

    #[test]
    fn test_get_worktree_path() {
        assert_eq!(
            get_worktree_path("T-0001"),
            PathBuf::from(".orc/worktrees/T-0001")
        );
    }
}
