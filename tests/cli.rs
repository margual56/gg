use gg::git_commands::*;
use git2::Repository;
use std::process::Command;
use tempfile::tempdir;

fn setup_git_repo(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    Command::new("git")
        .args(["init"])
        .current_dir(path)
        .status()?;
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .status()?;
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .status()?;
    Ok(())
}

#[test]
fn test_save_command() -> Result<(), Box<dyn std::error::Error>> {
    // Tests `gg save "My test commit"`
    // Equivalent to:
    // git add .
    // git commit -m "My test commit"

    // 1. Setup
    let temp_dir = tempdir()?;
    let repo_path = temp_dir.path();
    setup_git_repo(repo_path)?;
    let repo = Repository::open(repo_path)?;

    // 2. Create a new file and stage it, because commit_all no longer stages.
    std::fs::write(repo_path.join("test.txt"), "hello world")?;
    let mut index = repo.index()?;
    index.add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    // 3. Run the `commit_all` function, which is the core of `gg save`
    commit_all(&repo, "My test commit", false)?;

    // 4. Verify the result
    let log_output = Command::new("git")
        .args(["log", "-1", "--pretty=%B"])
        .current_dir(repo_path)
        .output()?;
    assert!(log_output.status.success());
    assert_eq!(
        String::from_utf8(log_output.stdout)?.trim(),
        "My test commit"
    );

    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()?;
    assert!(status_output.status.success());
    assert!(String::from_utf8(status_output.stdout)?.is_empty());

    Ok(())
}

#[test]
fn test_create_feature_with_base() -> Result<(), Box<dyn std::error::Error>> {
    // Tests `gg feature my-feature --base main`
    // Equivalent to:
    // git fetch origin main
    // git checkout -b my-feature origin/main
    // git push -u origin my-feature

    // 1. Setup a local repo with a remote
    let base_dir = tempdir()?;
    let remote_path = base_dir.path().join("remote.git");
    let local_path = base_dir.path().join("local");

    // Create a bare remote repo
    Command::new("git")
        .args(["init", "--bare"])
        .arg(&remote_path)
        .status()?;

    // Clone it to create a local repo
    Command::new("git")
        .args([
            "clone",
            &remote_path.to_str().unwrap(),
            &local_path.to_str().unwrap(),
        ])
        .status()?;
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["config", "push.default", "current"])
        .current_dir(&local_path)
        .status()?;

    // Create a commit on main and push
    std::fs::write(local_path.join("main1.txt"), "main 1")?;
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&local_path)
        .status()?;
    let main_head_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&local_path)
        .output()?;
    let main_head = String::from_utf8(main_head_output.stdout)?
        .trim()
        .to_string();

    // 2. Execute the `create_feature_branch` function
    let repo = Repository::open(&local_path)?;
    let feature_name = "my-feature";
    let base_branch_name = "main";
    create_feature_branch(&repo, feature_name, Some(base_branch_name.to_string()))?;

    // 3. Verify the result
    let current_branch = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&local_path)
            .output()?
            .stdout,
    )?
    .trim()
    .to_string();
    assert_eq!(current_branch, feature_name);

    let feature_head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&local_path)
            .output()?
            .stdout,
    )?
    .trim()
    .to_string();
    assert_eq!(feature_head, main_head);

    let remote_branch_output = Command::new("git")
        .args([
            "ls-remote",
            "origin",
            &format!("refs/heads/{}", feature_name),
        ])
        .current_dir(&local_path)
        .output()?;
    assert!(!String::from_utf8(remote_branch_output.stdout)?.is_empty());

    Ok(())
}

#[test]
fn test_create_feature_no_base() -> Result<(), Box<dyn std::error::Error>> {
    // Tests `gg feature my-feature`
    // Equivalent to:
    // git pull origin <current-branch>
    // git checkout -b my-feature
    // git push -u origin my-feature

    // 1. Setup
    let base_dir = tempdir()?;
    let remote_path = base_dir.path().join("remote.git");
    let local_path = base_dir.path().join("local");

    Command::new("git")
        .args(["init", "--bare"])
        .arg(&remote_path)
        .status()?;
    Command::new("git")
        .args([
            "clone",
            &remote_path.to_str().unwrap(),
            &local_path.to_str().unwrap(),
        ])
        .status()?;
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&local_path)
        .status()?;

    std::fs::write(local_path.join("main.txt"), "main")?;
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&local_path)
        .status()?;

    Command::new("git")
        .args(["checkout", "-b", "dev"])
        .current_dir(&local_path)
        .status()?;
    std::fs::write(local_path.join("dev.txt"), "dev")?;
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["commit", "-m", "Dev commit"])
        .current_dir(&local_path)
        .status()?;
    let dev_head_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&local_path)
        .output()?;
    let dev_head = String::from_utf8(dev_head_output.stdout)?
        .trim()
        .to_string();

    // 2. Execute
    let repo = Repository::open(&local_path)?;
    let feature_name = "my-feature";
    create_feature_branch(&repo, feature_name, None)?;

    // 3. Verify
    let current_branch = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&local_path)
            .output()?
            .stdout,
    )?
    .trim()
    .to_string();
    assert_eq!(current_branch, feature_name);

    let feature_head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&local_path)
            .output()?
            .stdout,
    )?
    .trim()
    .to_string();
    assert_eq!(feature_head, dev_head);

    let remote_branch_output = Command::new("git")
        .args([
            "ls-remote",
            "origin",
            &format!("refs/heads/{}", feature_name),
        ])
        .current_dir(&local_path)
        .output()?;
    assert!(!String::from_utf8(remote_branch_output.stdout)?.is_empty());

    Ok(())
}

// The 'done' function is not in the provided git_commands.rs, but I am adding the tests
// as requested ("like you did before"). These will fail to compile if 'done' does not exist.
#[test]
fn test_done() -> Result<(), Box<dyn std::error::Error>> {
    // Tests `gg done`
    // Equivalent to:
    // git checkout main
    // git pull origin main
    // git branch -d <feature-branch>

    // 1. Setup
    let base_dir = tempdir()?;
    let remote_path = base_dir.path().join("remote.git");
    let local_path = base_dir.path().join("local");

    Command::new("git")
        .args(["init", "--bare"])
        .arg(&remote_path)
        .status()?;
    Command::new("git")
        .args([
            "clone",
            &remote_path.to_str().unwrap(),
            &local_path.to_str().unwrap(),
        ])
        .status()?;
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&local_path)
        .status()?;

    std::fs::write(local_path.join("main.txt"), "main")?;
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&local_path)
        .status()?;

    let feature_name = "my-feature";
    Command::new("git")
        .args(["checkout", "-b", feature_name])
        .current_dir(&local_path)
        .status()?;
    std::fs::write(local_path.join("feature.txt"), "feature")?;
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["commit", "-m", "Feature commit"])
        .current_dir(&local_path)
        .status()?;

    // 2. Execute `done`
    let repo = Repository::open(&local_path)?;
    done(&repo, false)?;

    // 3. Verify
    let current_branch = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&local_path)
            .output()?
            .stdout,
    )?
    .trim()
    .to_string();
    assert_eq!(current_branch, "main");

    let branch_exists_output = Command::new("git")
        .args(["branch", "--list", feature_name])
        .current_dir(&local_path)
        .output()?;
    assert!(String::from_utf8(branch_exists_output.stdout)?.is_empty());

    Ok(())
}

#[test]
fn test_done_no_clean() -> Result<(), Box<dyn std::error::Error>> {
    // Tests `gg done --no-clean`
    // Equivalent to:
    // git checkout main
    // git pull origin main

    // 1. Setup
    let base_dir = tempdir()?;
    let remote_path = base_dir.path().join("remote.git");
    let local_path = base_dir.path().join("local");

    Command::new("git")
        .args(["init", "--bare"])
        .arg(&remote_path)
        .status()?;
    Command::new("git")
        .args([
            "clone",
            &remote_path.to_str().unwrap(),
            &local_path.to_str().unwrap(),
        ])
        .status()?;
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&local_path)
        .status()?;

    std::fs::write(local_path.join("main.txt"), "main")?;
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&local_path)
        .status()?;

    let feature_name = "my-feature";
    Command::new("git")
        .args(["checkout", "-b", feature_name])
        .current_dir(&local_path)
        .status()?;
    std::fs::write(local_path.join("feature.txt"), "feature")?;
    Command::new("git")
        .args(["add", "."])
        .current_dir(&local_path)
        .status()?;
    Command::new("git")
        .args(["commit", "-m", "Feature commit"])
        .current_dir(&local_path)
        .status()?;

    // 2. Execute `done` with `no_clean = true`
    let repo = Repository::open(&local_path)?;
    done(&repo, true)?;

    // 3. Verify
    let current_branch = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&local_path)
            .output()?
            .stdout,
    )?
    .trim()
    .to_string();
    assert_eq!(current_branch, "main");

    let branch_exists_output = Command::new("git")
        .args(["branch", "--list", feature_name])
        .current_dir(&local_path)
        .output()?;
    assert!(!String::from_utf8(branch_exists_output.stdout)?.is_empty());

    Ok(())
}
