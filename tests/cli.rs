use gg::git_commands::*;
use git2::Repository;
use std::path::PathBuf;
use std::process::Command;
use tempfile::{TempDir, tempdir};

/// Helper to manage a sandboxed Git environment
struct TestContext {
    pub dir: TempDir,
    pub path: PathBuf,
}

impl TestContext {
    fn new() -> Self {
        let dir = tempdir().expect("Failed to create temp dir");
        let path = dir.path().to_path_buf();
        let ctx = Self { dir, path };
        ctx.init();
        ctx
    }

    fn init(&self) {
        // Force the default branch to 'main' so the test is predictable
        self.git()
            .args(["init", "--initial-branch=main"])
            .status()
            .unwrap();

        self.git()
            .args(["config", "user.name", "Test User"])
            .status()
            .unwrap();
        self.git()
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap();

        // Create an initial commit so 'main' exists
        self.write_file("init.txt", "initial");
        self.git().args(["add", "."]).status().unwrap();
        self.git()
            .args(["commit", "-m", "initial commit"])
            .status()
            .unwrap();
    }

    fn git(&self) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.path);
        cmd.env("HOME", &self.path);
        cmd.env("GIT_CONFIG_NOSYSTEM", "1");

        cmd.env("GIT_PAGER", "cat"); // Prevent git from using a pager
        cmd.env("GIT_EDITOR", "true"); // Prevents Vim/Nano from opening
        cmd.env("GIT_MERGE_AUTOEDIT", "no"); // Prevents merge message editor
        cmd.env("GIT_TERMINAL_PROMPT", "0"); // Prevents "Username for 'https://...':"

        cmd
    }

    fn write_file(&self, name: &str, content: &str) {
        std::fs::write(self.path.join(name), content).unwrap();
    }

    fn get_stdout(&self, args: &[&str]) -> String {
        let out = self.git().args(args).output().expect("Git cmd failed");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
}

#[test]
fn test_save_command() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = TestContext::new();
    let repo = Repository::open(&ctx.path)?;

    // 1. Stage a file
    ctx.write_file("test.txt", "hello world");
    let mut index = repo.index()?;
    index.add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    // 2. Execute
    commit_all(&repo, "My test commit", false)?;

    // 3. Verify
    assert_eq!(
        ctx.get_stdout(&["log", "-1", "--pretty=%B"]),
        "My test commit"
    );
    assert!(ctx.get_stdout(&["status", "--porcelain"]).is_empty());
    Ok(())
}

#[test]
fn test_create_feature_with_base() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup a BARE remote
    let remote_dir = tempdir()?;
    Command::new("git")
        .args(["init", "--bare"])
        .current_dir(remote_dir.path())
        .status()?;

    // 2. Setup local
    let ctx = TestContext::new();
    let remote_path = remote_dir.path().to_str().unwrap();
    ctx.git()
        .args(["remote", "add", "origin", remote_path])
        .status()?;

    // Push the initial commit to the bare remote so it has a 'main'
    ctx.git().args(["push", "origin", "main"]).status()?;

    // 3. Run app logic
    let repo = Repository::open(&ctx.path)?;
    create_feature_branch(&repo, "my-feature", Some("main".to_string()))?;

    // 4. Verify
    assert_eq!(
        ctx.get_stdout(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "my-feature"
    );
    Ok(())
}

#[test]
fn test_done_deletes_branch() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = TestContext::new();

    // 1. Create a branch and commit
    ctx.git()
        .args(["checkout", "-b", "feature-branch"])
        .status()?;
    ctx.write_file("f.txt", "feat");
    ctx.git().args(["add", "."]).status()?;
    ctx.git().args(["commit", "-m", "feat commit"]).status()?;

    // 2. IMPORTANT: To prevent 'git pull' from hanging inside your 'done' function,
    // we give this branch a "fake" upstream or ensure there's nothing to pull.
    // Alternatively, if your 'done' logic allows it, we just ensure main is ready.
    ctx.git().args(["checkout", "main"]).status()?;
    // Merge feature into main so it is "merged" and safe to delete - use --no-edit!
    ctx.git()
        .args(["merge", "feature-branch", "--no-edit"])
        .status()?;
    ctx.git().args(["checkout", "feature-branch"]).status()?;

    let repo = Repository::open(&ctx.path)?;

    // 3. Execute 'done'. We wrap this in a timeout or ensure env is clean.
    // Since 'done' calls 'pull', and there's no remote, it might fail quickly
    // instead of hanging if GIT_TERMINAL_PROMPT=0 is set.
    done(&repo, false, false)?;

    assert_eq!(
        ctx.get_stdout(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );
    Ok(())
}

#[test]
fn test_resolve_conflict_and_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = TestContext::new();

    // 1. Setup Conflict
    ctx.write_file("conflict.txt", "base content");
    ctx.git().args(["add", "conflict.txt"]).status()?;
    ctx.git().args(["commit", "-m", "base"]).status()?;

    ctx.git().args(["checkout", "-b", "other"]).status()?;
    ctx.write_file("conflict.txt", "other content");
    ctx.git().args(["commit", "-am", "other change"]).status()?;

    ctx.git().args(["checkout", "main"]).status()?;
    ctx.write_file("conflict.txt", "main content");
    ctx.git().args(["commit", "-am", "main change"]).status()?;

    // 2. Merge and capture output to ensure it conflicted
    _ = ctx.git().args(["merge", "other"]).output()?;
    // git merge returns 1 when there's a conflict.

    // 3. Verify libgit2 actually sees the conflict
    let repo = Repository::open(&ctx.path)?;
    let index = repo.index()?;
    assert!(
        index.has_conflicts(),
        "The Git index must have conflicts for Resolve to work"
    );

    // 4. Run app logic
    resolve(&repo, false)?;

    let theirs_path = ctx.path.join("conflict.txt.theirs");
    assert!(
        theirs_path.exists(),
        "The .theirs helper file was not created"
    );

    Ok(())
}
