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
