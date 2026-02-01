use std::{
    io::{self, Write},
    path::Path,
};

use git2::{BranchType, Error, PushOptions, Repository};
use owo_colors::OwoColorize;

use crate::helpers::{create_callbacks, has_remote, show_progress};

pub fn commit_all(repo: &Repository, message: &str, amend: bool) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    let oid = index.write_tree()?;
    let tree = repo.find_tree(oid)?;
    let signature = repo.signature()?;

    let head_ref = repo.head().ok();
    let head_commit = head_ref.as_ref().and_then(|h| h.peel_to_commit().ok());

    let (final_message, parents) = if amend {
        let parent = head_commit.ok_or_else(|| git2::Error::from_str("No commit to amend"))?;

        // Use the existing commit's message if amending
        let msg = parent.message().unwrap_or(message);

        // When amending, the parents are the parents of the commit we are replacing
        let p: Vec<_> = parent.parents().collect();
        (msg.to_string(), p)
    } else {
        let mut p = Vec::new();
        if let Some(ref parent) = head_commit {
            p.push(parent.clone());
        }
        (message.to_string(), p)
    };

    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

    // To prevent "current tip is not the first parent" error:
    // If we are NOT amending, we update HEAD automatically.
    // If we ARE amending, we create the commit without updating the ref immediately,
    // then manually update the reference to point to the new commit.
    let update_ref = if amend { None } else { Some("HEAD") };

    let new_commit_id = repo.commit(
        update_ref,
        &signature,
        &signature,
        &final_message,
        &tree,
        &parent_refs,
    )?;

    if amend {
        // Manually update the current branch reference to the new commit
        if let Some(mut head) = head_ref {
            head.set_target(new_commit_id, "gg: amend commit")?;
        }
    }

    Ok(())
}

/// Helper to Push changes to remote
pub fn push(
    repo: &Repository,
    remote_name: &str,
    branch_name: &str,
    force: bool,
) -> Result<(), Error> {
    // Safety check: Never try to push a literal "HEAD" refspec
    if branch_name == "HEAD" {
        return Err(Error::from_str(
            "Cannot push 'HEAD'. You must be on a named branch.",
        ));
    }

    if !has_remote(repo, remote_name) {
        return Ok(());
    }

    let mut remote = repo.find_remote(remote_name)?;
    let mut push_opts = PushOptions::new();
    push_opts.remote_callbacks(create_callbacks());

    let prefix = if force { "+" } else { "" };
    let refspec = format!("{prefix}refs/heads/{branch_name}:refs/heads/{branch_name}");

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
        let refname = format!("refs/heads/{branch_name}");
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
            resolve_conflicts_ours(repo, &mut index)?;
            println!("\nYou can manually merge the '.theirs' files at any time.");
        }

        // Now, create the merge commit. If there were conflicts, this commit will
        // contain the 'ours' versions that we added back to the index.
        let tree_oid = index.write_tree_to(repo)?;
        let tree = repo.find_tree(tree_oid)?;

        let signature = repo.signature()?;
        let head_shorthand = repo.head()?.shorthand().unwrap_or("HEAD").to_string();
        let msg =
            format!("Merge remote-tracking branch 'origin/{head_shorthand}' into {head_shorthand}");

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

fn resolve_conflicts_ours(repo: &Repository, index: &mut git2::Index) -> Result<(), Error> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| Error::from_str("Repository has no workdir"))?;

    println!("\n--- Conflicts detected. Auto-resolving using local version. ---");
    println!("--- Remote changes saved to '.theirs' files for later review: ---");

    let conflicts: Vec<_> = index.conflicts()?.filter_map(Result::ok).collect();

    for conflict in conflicts {
        let our_path_str: String;

        // First, resolve the conflict in the index by choosing our version.
        if let Some(our) = &conflict.our {
            let path_bytes = &our.path;
            our_path_str = String::from_utf8_lossy(path_bytes).to_string();
            let path = Path::new(&our_path_str);

            let blob = repo.find_blob(our.id)?;
            let full_path = workdir.join(path);

            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| Error::from_str(&format!("Failed to create dirs: {e}")))?;
            }
            std::fs::write(&full_path, blob.content())
                .map_err(|e| Error::from_str(&format!("Failed to write file: {e}")))?;

            index.add_path(path)?;
        } else {
            continue;
        }

        // Second, save the 'theirs' version to a file.
        if let Some(their) = &conflict.their {
            let blob = repo.find_blob(their.id)?;
            let content = blob.content();
            let theirs_filename = format!("{our_path_str}.theirs");
            let theirs_path = workdir.join(&theirs_filename);

            if let Err(e) = std::fs::write(&theirs_path, content) {
                eprintln!(
                    "Warning: Could not write remote changes to {}: {e}",
                    theirs_path.display()
                );
            } else {
                println!(
                    "  - Remote version of {our_path_str} saved to {}",
                    theirs_path.display()
                );
            }
        } else {
            println!("  - {our_path_str} (conflict: remote version was deleted or not present)");
        }
    }

    Ok(())
}

pub fn resolve(repo: &Repository, cleanup: bool) -> Result<(), Error> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| Error::from_str("Repository has no workdir"))?;

    if cleanup {
        let mut theirs_files = Vec::new();
        find_theirs_files(workdir, &mut theirs_files)
            .map_err(|e| Error::from_str(&format!("Error scanning for conflict files: {e}")))?;

        if theirs_files.is_empty() {
            println!("No conflict files (.theirs) to clean up.");
        } else {
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
        }
        return Ok(());
    }

    let mut index = repo.index()?;
    if index.has_conflicts() {
        resolve_conflicts_ours(repo, &mut index)?;
        index.write()?;
        println!("\nYou can manually merge the '.theirs' files at any time.");
        println!("When you are done, run 'gg resolve --cleanup' to remove the .theirs files.");
    } else {
        let mut theirs_files = Vec::new();
        find_theirs_files(workdir, &mut theirs_files)
            .map_err(|e| Error::from_str(&format!("Error scanning for conflict files: {e}")))?;

        if theirs_files.is_empty() {
            println!("No conflicts found to resolve.");
        } else {
            println!("--- Conflicts to resolve ---");
            println!("The following files have saved remote changes:");
            for path in theirs_files {
                let theirs_path_str = path.to_string_lossy();
                let original_path_str = theirs_path_str.trim_end_matches(".theirs");
                println!("  - {original_path_str} (remote saved to {theirs_path_str})");
            }
            println!("\nPlease use your preferred merge tool to combine them. For example:");
            println!("  code --diff path/to/your/file path/to/your/file.theirs");
            println!("  vimdiff path/to/your/file path/to/your/file.theirs");
            println!(
                "\nWhen you are done, run 'gg resolve --cleanup' to remove the .theirs files."
            );
        }
    }

    Ok(())
}

pub fn create_feature_branch(
    repo: &git2::Repository,
    name: &str,
    base: Option<String>,
) -> Result<(), Error> {
    // 1. Determine base commit
    let (base_commit, base_name) = match base {
        Some(base_branch_name) => {
            let commit = show_progress(
                &format!("Fetching latest of '{}'", base_branch_name.bold()),
                || {
                    let mut remote = repo.find_remote("origin")?;
                    let mut fetch_opts = git2::FetchOptions::new();
                    fetch_opts.remote_callbacks(create_callbacks());
                    remote.fetch(&[&base_branch_name], Some(&mut fetch_opts), None)?;

                    let fetch_head = repo.find_reference("FETCH_HEAD")?;
                    let commit = repo.reference_to_annotated_commit(&fetch_head)?.id();
                    repo.find_commit(commit)
                },
            )?;
            println!("Basing new feature on '{}'", base_branch_name.bold());
            (commit, base_branch_name)
        }
        None => {
            show_progress("Syncing current branch", || pull(repo, "origin", "HEAD"))?;
            let commit = repo.head()?.peel_to_commit()?;
            (commit, "HEAD".to_string())
        }
    };

    // 2. Create or switch to branch
    let branch = if let Ok(b) = repo.find_branch(name, BranchType::Local) {
        b
    } else if let Ok(remote_branch) =
        repo.find_branch(&format!("origin/{name}"), BranchType::Remote)
    {
        show_progress("Creating local tracking branch", || {
            let commit = remote_branch.get().peel_to_commit()?;
            let mut branch = repo.branch(name, &commit, false)?;
            branch.set_upstream(Some(&format!("origin/{name}")))?;
            Ok(branch)
        })?
    } else {
        println!(
            "Creating feature branch '{}' from {}",
            name.bold(),
            base_name.bold()
        );
        repo.branch(name, &base_commit, false)?
    };

    // 3. Switch HEAD
    if repo.head()?.shorthand() != Some(name) {
        show_progress(&format!("Switching to branch '{}'", name.bold()), || {
            let refname = branch
                .get()
                .name()
                .ok_or_else(|| Error::from_str("Branch refname not found"))?;
            repo.set_head(refname)?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::default().safe()))
        })?;
    } else {
        println!("Already on branch '{}'", name.bold());
    }

    // 4. Push upstream
    show_progress("Pushing upstream", || push(repo, "origin", name, false))?;

    Ok(())
}

pub fn done(repo: &Repository, no_clean: bool, confirm_deletion: bool) -> Result<(), Error> {
    let head = repo.head()?;
    let current_branch_name = head
        .shorthand()
        .ok_or_else(|| Error::from_str("Not on a valid branch"))?
        .to_string();

    let main_branch = if repo.find_branch("main", BranchType::Local).is_ok() {
        "main"
    } else {
        "master"
    };

    if current_branch_name == main_branch {
        println!("Already on {main_branch}, nothing to finalize.");
        return Ok(());
    }

    show_progress(&format!("Switching to {main_branch}"), || {
        repo.set_head(&format!("refs/heads/{main_branch}"))?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().safe()))
    })?;

    show_progress(&format!("Pulling {main_branch}"), || {
        pull(repo, "origin", main_branch)
    })?;

    if !no_clean {
        // Check if the branch exists on the remote (usually 'origin')
        let remote_branch_exists = repo
            .find_branch(&format!("origin/{current_branch_name}"), BranchType::Remote)
            .is_ok();

        if confirm_deletion && !remote_branch_exists {
            print!(
                "\n⚠️  {}: Branch '{}' has not been pushed to remote.\n\
                Deleting it will result in permanent data loss. \n\
                Proceed anyway? [y/N]: ",
                "Warning".yellow(),
                current_branch_name.bold()
            );
            _ = io::stdout().flush();

            let mut input = String::new();
            io::stdin().read_line(&mut input).unwrap();
            let response = input.trim().to_lowercase();

            if response != "y" && response != "yes" {
                println!("Operation aborted. Keeping branch {current_branch_name}.");
                return Ok(());
            }
        }

        show_progress(&format!("Deleting branch {current_branch_name}"), || {
            let mut branch = repo.find_branch(&current_branch_name, BranchType::Local)?;
            branch.delete()
        })?;
    }

    Ok(())
}
