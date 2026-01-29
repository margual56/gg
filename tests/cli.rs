use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_save_command() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup: Create a temporary directory and initialize a git repo
    let temp_dir = tempdir()?;
    let repo_path = temp_dir.path();

    Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .status()?;

    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(repo_path)
        .status()?;

    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo_path)
        .status()?;

    // 2. Create a new file
    std::fs::write(repo_path.join("test.txt"), "hello world")?;

    // 3. Run the `gg save` command
    let mut cmd = Command::cargo_bin("gg")?;
    cmd.arg("save")
        .arg("-m")
        .arg("My test commit")
        .current_dir(repo_path);

    cmd.assert().success().stdout(predicate::str::contains(
        "--- Committing: \"My test commit\" ---",
    ));

    // 4. Verify the result
    // Check the last commit message
    let log_output = Command::new("git")
        .args(["log", "-1", "--pretty=%B"])
        .current_dir(repo_path)
        .output()?;

    assert!(log_output.status.success());
    assert_eq!(
        String::from_utf8(log_output.stdout)?.trim(),
        "My test commit"
    );

    // Check that the repo is clean
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()?;

    assert!(status_output.status.success());
    assert!(String::from_utf8(status_output.stdout)?.is_empty());

    Ok(())
}
