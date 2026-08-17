use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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
            return Ok((branch, worktree_path));
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
        let output = Command::new("git")
            .current_dir(git_dir)
            .arg("worktree")
            .arg("add")
            .arg(&worktree_path)
            .arg(&branch)
            .output()
            .context("failed to execute git worktree add with existing branch")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed for task {}: {}", task_id, stderr);
        }
        Ok((branch, worktree_path))
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

/// Show the diff for a task worktree/branch.
/// Returns the diff output as a string.
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

    // Get the base branch (usually main/master)
    let output = Command::new("git")
        .current_dir(&absolute_worktree_path)
        .arg("diff")
        .arg("HEAD^..HEAD")
        .output()
        .context("failed to get diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(stderr.to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
