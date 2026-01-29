use git2::{Error, PushOptions, Repository};

use crate::helpers::{create_callbacks, has_remote};

pub fn commit_all(repo: &Repository, message: &str) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    let oid = index.write_tree()?;
    let tree = repo.find_tree(oid)?;
    let signature = repo.signature()?;

    // Check if HEAD exists to find parent; if not, it's an empty list (Initial Commit)
    let parent_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let mut parents = Vec::new();
    if let Some(ref p) = parent_commit {
        parents.push(p);
    }

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )?;

    Ok(())
}

/// Helper to Push changes to remote
pub fn push(repo: &Repository, remote_name: &str, branch_name: &str) -> Result<(), Error> {
    if !has_remote(repo, remote_name) {
        return Ok(());
    }

    let mut remote = repo.find_remote(remote_name)?;

    let mut push_opts = PushOptions::new();
    push_opts.remote_callbacks(create_callbacks());

    // Refspec: refs/heads/branch:refs/heads/branch
    let refspec = format!("refs/heads/{}:refs/heads/{}", branch_name, branch_name);

    remote.push(&[&refspec], Some(&mut push_opts))?;

    Ok(())
}

/// Helper to Pull (Fetch + Merge/FastForward)
/// Note: git2 does not have a "pull" command. We must Fetch, Analyze, then Merge.
pub fn pull(repo: &Repository, remote_name: &str, branch_name: &str) -> Result<(), Error> {
    if !has_remote(repo, remote_name) {
        return Ok(());
    }

    // 1. Fetch
    let mut remote = repo.find_remote(remote_name)?;
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(create_callbacks());

    // Fetch specifically the branch we are interested in, or HEAD
    remote.fetch(&[branch_name], Some(&mut fetch_opts), None)?;

    // 2. Prepare for Merge Analysis
    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let fetch_commit = repo.reference_to_annotated_commit(&fetch_head)?;

    let analysis = repo.merge_analysis(&[&fetch_commit])?;

    // 3. Act on Analysis
    if analysis.0.is_fast_forward() {
        // Fast-forward the branch
        let refname = format!("refs/heads/{}", branch_name);
        match repo.find_reference(&refname) {
            Ok(mut r) => {
                let name = refname.clone();
                let msg = format!(
                    "Fast-Forward: Setting {} to id: {}",
                    name,
                    fetch_commit.id()
                );
                r.set_target(fetch_commit.id(), &msg)?;
                repo.set_head(&name)?;
                repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
            }
            Err(_) => {
                // If checking HEAD and it's detached or similar, just checkout the commit
                repo.set_head_detached(fetch_commit.id())?;
            }
        };
    } else if analysis.0.is_up_to_date() {
        // Do nothing
    } else if analysis.0.is_normal() {
        // For this simple CLI tool, we prevent complex merge conflicts by failing
        return Err(Error::from_str(
            "Pull requires a merge commit. Please merge manually to resolve conflicts.",
        ));
    }

    Ok(())
}

pub fn sync_remote(repo: &Repository, remote_name: &str) -> Result<(), Error> {
    // 1. Fetch the remote to see what's there
    let mut remote = repo.find_remote(remote_name)?;
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(create_callbacks());
    remote.fetch(
        &["+refs/heads/*:refs/remotes/origin/*"],
        Some(&mut fetch_opts),
        None,
    )?;
    println!();

    // 2. Identify current local branch
    let head = repo.head()?;
    let local_branch_name = head.shorthand().unwrap_or("main");
    let mut local_branch = repo.find_branch(local_branch_name, git2::BranchType::Local)?;

    // 3. Try to find the corresponding remote branch (e.g., origin/main)
    let remote_branch_name = format!("{}/{}", remote_name, local_branch_name);

    // 4. Set the Upstream (The "weird shit" fix)
    // This links the local branch to the remote so `git pull` works without args later
    local_branch.set_upstream(Some(&remote_branch_name))?;

    // 5. If we have a remote branch, try a safe merge/fast-forward
    let remote_ref_name = format!("refs/remotes/{}", remote_branch_name);
    if let Ok(remote_ref) = repo.find_reference(&remote_ref_name) {
        let fetch_commit = repo.reference_to_annotated_commit(&remote_ref)?;
        let (analysis, _) = repo.merge_analysis(&[&fetch_commit])?;

        if analysis.is_fast_forward() {
            // If local is behind, just catch up
            let log_message = format!("Fast-forward to {}", remote_branch_name);
            local_branch
                .get_mut()
                .set_target(fetch_commit.id(), &log_message)?;
            repo.set_head(local_branch.get().name().unwrap())?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
        } else if analysis.is_up_to_date() {
            // Already synced
        } else {
            // This is the "Unrelated Histories" or "Diverged" case.
            // For safety, we notify the user rather than forcing a destructive rebase.
            return Err(Error::from_str(
                "Remote has conflicting commits. Use 'Save' to commit local work first.",
            ));
        }
    }

    Ok(())
}
