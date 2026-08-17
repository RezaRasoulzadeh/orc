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
        // This test uses a temporary git repository
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let repo_path = tmpdir.path();

        // Initialize a git repo
        Command::new("git")
            .current_dir(repo_path)
            .arg("init")
            .arg(".")
            .output()
            .expect("init repo");

        // Configure git user for commit operations
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

        // Create initial commit
        let file_path = repo_path.join("README.md");
        std::fs::write(&file_path, "test").expect("write file");
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

        // Now create a worktree
        let (branch, worktree_path) =
            create_worktree("T-0001", repo_path).expect("create worktree");

        assert_eq!(branch, "orc/task/T-0001");
        assert_eq!(worktree_path, PathBuf::from(".orc/worktrees/T-0001"));

        // Verify worktree exists
        let absolute_worktree_path = repo_path.join(&worktree_path);
        assert!(
            absolute_worktree_path.exists(),
            "worktree path should exist"
        );

        // Verify we're on the right branch in the worktree
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
