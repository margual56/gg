use git2::{Cred, Error, RemoteCallbacks, Repository};
use std::io::Write;

pub fn has_remote(repo: &Repository, name: &str) -> bool {
    repo.find_remote(name).is_ok()
}

/// Analyzes the diff to suggest a Conventional Commit prefix
pub fn generate_conventional_message(repo: &Repository) -> Result<String, git2::Error> {
    let mut index = repo.index()?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    let head_tree = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?.tree()?),
        Err(_) => None,
    };

    let diff = repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), None)?;

    let mut added = 0;
    let mut deleted = 0;
    let mut modified = 0;
    let mut affected_files = Vec::new();

    diff.foreach(
        &mut |delta, _| {
            let path = delta.new_file().path().or(delta.old_file().path());
            if let Some(p) = path {
                affected_files.push(p.to_string_lossy().into_owned());
            }
            match delta.status() {
                git2::Delta::Added => added += 1,
                git2::Delta::Deleted => deleted += 1,
                git2::Delta::Modified => modified += 1,
                _ => {}
            }
            true
        },
        None,
        None,
        None,
    )?;

    if affected_files.is_empty() {
        return Ok("chore: no changes detected".to_string());
    }

    // 1. Determine the Verb and Prefix
    let (prefix, verb) = if added > 0 && modified == 0 && deleted == 0 {
        ("feat", "added")
    } else if deleted > 0 && added == 0 && modified == 0 {
        ("fix", "removed")
    } else if modified > 0 && added == 0 && deleted == 0 {
        ("fix", "changed")
    } else {
        ("fix", "updated") // Mixed changes
    };

    // 2. Format the message
    if affected_files.len() == 1 {
        let file = &affected_files[0];
        let p = if file.ends_with(".md") || file.contains("docs/") {
            "docs"
        } else {
            prefix
        };
        Ok(format!(
            "{}({}): {} file (+{}, -{}, ~{})",
            p, file, verb, added, deleted, modified
        ))
    } else {
        Ok(format!(
            "{}: {} {} files (+{}, -{}, ~{})",
            prefix,
            verb,
            affected_files.len(),
            added,
            deleted,
            modified
        ))
    }
}

/// Creates remote callbacks for SSH/Credential handling
pub fn create_callbacks() -> RemoteCallbacks<'static> {
    let mut callbacks = RemoteCallbacks::new();

    // Handle SSH Key Authentication
    callbacks.credentials(|_url, username_from_url, _allowed_types| {
        let user = username_from_url.unwrap_or("git");
        println!("--- Authenticating as '{}' ---", user);
        Cred::ssh_key_from_agent(user)
    });

    // Handle Host Key Verification (The "Hang" Fix)
    callbacks.certificate_check(|_cert, _host| {
        // FIXME: In a production tool, you'd verify the host against known_hosts.
        // Returning true tells libgit2 to trust the host and proceed.
        Ok(git2::CertificateCheckStatus::CertificateOk)
    });

    // Show Progress (Visual feedback)
    callbacks.transfer_progress(|stats| {
        if stats.received_objects() > 0 {
            print!(
                "\rFetching: {}/{} objects ({})",
                stats.received_objects(),
                stats.total_objects(),
                stats.indexed_objects()
            );
            let _ = std::io::stdout().flush();
        }
        true
    });

    callbacks
}

pub fn configure_git(
    repo: &Repository,
    name: &str,
    email: &str,
    global: bool,
) -> Result<(), Error> {
    let mut config = if global {
        // Access the global ~/.gitconfig
        git2::Config::open_default()?
    } else {
        // Access the repo-specific .git/config
        repo.config()?
    };

    config.set_str("user.name", name)?;
    config.set_str("user.email", email)?;

    Ok(())
}

pub fn setup_remote(repo: &Repository, name: &str, url: &str) -> Result<(), Error> {
    // Check if the remote already exists
    match repo.find_remote(name) {
        Ok(_) => {
            // If it exists, we update the URL
            repo.remote_set_url(name, url)?;
        }
        Err(_) => {
            // If it doesn't exist, we create it
            repo.remote(name, url)?;
        }
    }
    Ok(())
}

pub fn is_dirty(repo: &Repository) -> Result<bool, Error> {
    let mut status_options = git2::StatusOptions::new();
    // We include untracked files because they can cause conflicts during
    // branch switches or rebases.
    status_options.include_untracked(true);
    status_options.recurse_untracked_dirs(true);

    let statuses = repo.statuses(Some(&mut status_options))?;

    // If statuses is not empty, the repo is "dirty"
    Ok(!statuses.is_empty())
}
