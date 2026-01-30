use std::path::Path;

use git2::{Error, PushOptions, Repository};

use crate::helpers::{create_callbacks, has_remote};

pub fn commit_all(repo: &Repository, message: &str) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    index.add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)?;
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
        println!("--- Merging changes ---");

        let our_ref = repo.head()?;
        let our_commit = repo.reference_to_annotated_commit(&our_ref)?;

        let merge_base_oid = repo.merge_base(our_commit.id(), fetch_commit.id())?;
        let base_commit = repo.find_commit(merge_base_oid)?;

        let our_commit_obj = repo.find_commit(our_commit.id())?;
        let their_commit_obj = repo.find_commit(fetch_commit.id())?;

        let mut index = repo.merge_trees(
            &base_commit.tree()?,
            &our_commit_obj.tree()?,
            &their_commit_obj.tree()?,
            None,
        )?;

        if index.has_conflicts() {
            println!("\n--- Conflicts detected. Auto-resolved using local version. ---");
            println!("--- Remote changes saved to '.theirs' files for later review: ---");

            let conflicts: Vec<_> = index.conflicts()?.filter_map(Result::ok).collect();

            for conflict in conflicts {
                let our_path_str: String;

                // First, resolve the conflict in the index by choosing our version.
                if let Some(our) = &conflict.our {
                    let path_bytes = &our.path;
                    our_path_str = String::from_utf8_lossy(path_bytes).to_string();
                    let path = Path::new(&our_path_str);

                    // To resolve the conflict, we'll write our version of the file
                    // to the working directory and then add it to the index.
                    let blob = repo.find_blob(our.id)?;
                    let workdir = repo
                        .workdir()
                        .ok_or_else(|| Error::from_str("No workdir found"))?;
                    let full_path = workdir.join(path);

                    // Ensure parent directory exists
                    if let Some(parent) = full_path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            Error::from_str(&format!("Failed to create dirs: {}", e))
                        })?;
                    }

                    std::fs::write(&full_path, blob.content())
                        .map_err(|e| Error::from_str(&format!("Failed to write file: {}", e)))?;

                    // Add the resolved file to the index, which removes the conflict entry.
                    index.add_path(path)?;
                } else {
                    // This case is unlikely, but if a conflict exists without a local
                    // version, we can't resolve it this way.
                    continue;
                }

                // Second, save the 'theirs' version to a file.
                if let Some(their) = &conflict.their {
                    let blob = repo.find_blob(their.id)?;
                    let content = blob.content();
                    let theirs_path = format!("{}.theirs", our_path_str);

                    if let Err(e) = std::fs::write(&theirs_path, content) {
                        eprintln!(
                            "Warning: Could not write remote changes to {}: {}",
                            theirs_path, e
                        );
                    } else {
                        println!(
                            "  - Remote version of {} saved to {}",
                            our_path_str, theirs_path
                        );
                    }
                } else {
                    // This can happen if the conflict is a delete/modify.
                    println!(
                        "  - {} (conflict: remote version was deleted or not present)",
                        our_path_str
                    );
                }
            }
            println!("\nYou can manually merge the '.theirs' files at any time.");
        }

        // Now, create the merge commit. If there were conflicts, this commit will
        // contain the 'ours' versions that we added back to the index.
        let tree_oid = index.write_tree_to(repo)?;
        let tree = repo.find_tree(tree_oid)?;

        let signature = repo.signature()?;
        let head_shorthand = repo.head()?.shorthand().unwrap_or("HEAD").to_string();
        let msg = format!(
            "Merge remote-tracking branch 'origin/{}' into {}",
            head_shorthand, head_shorthand
        );

        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &msg,
            &tree,
            &[&our_commit_obj, &their_commit_obj],
        )?;

        // Finally, update the working directory to reflect the new merge commit
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
    }

    Ok(())
}

// --- Helper Functions ---

fn find_theirs_files(
    dir: &std::path::Path,
    found_files: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    if dir.is_dir() {
        // Ignore the .git directory
        if dir.ends_with(".git") {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                find_theirs_files(&path, found_files)?;
            } else if path.to_string_lossy().ends_with(".theirs") {
                found_files.push(path);
            }
        }
    }
    Ok(())
}

pub fn resolve(repo: &Repository, cleanup: bool) -> Result<(), Error> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| Error::from_str("Repository has no workdir"))?;
    let mut theirs_files = Vec::new();
    find_theirs_files(workdir, &mut theirs_files)
        .map_err(|e| Error::from_str(&format!("Error scanning for conflict files: {}", e)))?;

    if theirs_files.is_empty() {
        if cleanup {
            println!("No conflict files (.theirs) to clean up.");
        } else {
            println!("No conflicts found to resolve.");
        }
        return Ok(());
    }

    if cleanup {
        println!("--- Cleaning up resolved conflict files ---");
        for path in theirs_files {
            match std::fs::remove_file(&path) {
                Ok(_) => println!("  - Deleted {}", path.to_string_lossy()),
                Err(e) => eprintln!(
                    "Warning: Could not delete {}: {}",
                    path.to_string_lossy(),
                    e
                ),
            }
        }
    } else {
        println!("--- Conflicts to resolve ---");
        println!("The following files have saved remote changes:");
        for path in theirs_files {
            let theirs_path_str = path.to_string_lossy();
            let original_path_str = theirs_path_str.trim_end_matches(".theirs");
            println!(
                "  - {} (remote saved to {})",
                original_path_str, theirs_path_str
            );
        }
        println!("\nPlease use your preferred merge tool to combine them. For example:");
        println!("  code --diff path/to/your/file path/to/your/file.theirs");
        println!("  vimdiff path/to/your/file path/to/your/file.theirs");
        println!("\nWhen you are done, run 'gg resolve --cleanup' to remove the .theirs files.");
    }

    Ok(())
}
