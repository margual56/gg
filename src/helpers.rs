use git_url_parse::GitUrl;
use git_url_parse::types::provider::{AzureDevOpsProvider, GenericProvider, GitLabProvider};
use git2::{CertificateCheckStatus, Config, Cred, Error, RemoteCallbacks, Repository};
use owo_colors::OwoColorize;
use std::cell::Cell;
use std::io::Write;

pub fn has_remote(repo: &Repository, name: &str) -> bool {
    repo.find_remote(name).is_ok()
}

/// Analyzes the diff to suggest a Conventional Commit prefix
pub fn generate_conventional_message(repo: &Repository) -> Result<String, git2::Error> {
    let index = repo.index()?;

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
            "{p}({file}): {verb} file (+{added}, -{deleted}, ~{modified})"
        ))
    } else {
        Ok(format!(
            "{prefix}: {verb} {} files (+{added}, -{deleted}, ~{modified})",
            affected_files.len(),
        ))
    }
}

/// Creates remote callbacks for SSH/Credential handling
pub fn create_callbacks() -> RemoteCallbacks<'static> {
    let mut callbacks = RemoteCallbacks::new();
    let attempt = Cell::new(0);

    callbacks.credentials(move |url, username_from_url, allowed_types| {
        let count = attempt.get();
        attempt.set(count + 1);

        // Stop the infinite loop if we've tried agent, disk keys, and failed.
        if count > 2 {
            return Err(Error::from_str(
                "Authentication failed: tried agent and default SSH keys.",
            ));
        }

        let user = username_from_url.unwrap_or("git");

        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            if count == 0 {
                return Cred::ssh_key_from_agent(user);
            } else {
                // Fallback to common disk keys
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                let dot_ssh = std::path::Path::new(&home).join(".ssh");

                for key_name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
                    let key_path = dot_ssh.join(key_name);
                    if key_path.exists() {
                        return Cred::ssh_key(user, None, &key_path, None);
                    }
                }
            }
        }

        // If it's HTTPS, this usually pops a helper or fails for manual token entry
        if allowed_types.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            return Cred::credential_helper(&Config::open_default()?, url, username_from_url);
        }

        Err(Error::from_str("No valid authentication methods found"))
    });

    callbacks.certificate_check(|_cert, _host| Ok(CertificateCheckStatus::CertificateOk));
    callbacks
}

pub fn sync_unrelated_histories(repo: &Repository, remote_name: &str) -> Result<(), Error> {
    let mut remote = repo.find_remote(remote_name)?;
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(create_callbacks());

    // Fetch to see what the remote has
    remote.fetch(
        &["refs/heads/*:refs/remotes/origin/*"],
        Some(&mut fetch_opts),
        None,
    )?;

    // Determine local branch name (usually 'main' or 'master')
    let local_branch_name = repo
        .head()
        .map(|h| h.shorthand().unwrap_or("main").to_string())
        .unwrap_or_else(|_| "main".to_string());

    let remote_ref_name = format!("refs/remotes/{remote_name}/{local_branch_name}");

    if let Ok(remote_ref) = repo.find_reference(&remote_ref_name) {
        let remote_commit_annotated = repo.reference_to_annotated_commit(&remote_ref)?;
        // To get the actual Commit object:
        let remote_commit_actual = repo.find_commit(remote_commit_annotated.id())?;

        match repo.head() {
            Ok(head) => {
                let local_commit_annotated = repo.reference_to_annotated_commit(&head)?;
                if local_commit_annotated.id() != remote_commit_annotated.id() {
                    println!("--- Rebasing local work onto {remote_ref_name} ---");
                    let mut rebase =
                        repo.rebase(None, Some(&remote_commit_annotated), None, None)?;

                    while let Some(op) = rebase.next() {
                        op?;
                        if repo.index()?.has_conflicts() {
                            // Abort the rebase so the repo isn't left in a messy state
                            rebase.abort()?;
                            return Err(Error::from_str(
                                "Conflict! Rebase aborted. Resolve manually.",
                            ));
                        }
                        let sig = repo.signature()?;
                        rebase.commit(None, &sig, None)?;
                    }
                    rebase.finish(None)?;
                }
            }
            Err(_) => {
                println!("--- Initializing local branch from remote ---");
                // Use the actual commit object found via ID
                repo.branch(&local_branch_name, &remote_commit_actual, false)?;
                repo.set_head(&format!("refs/heads/{local_branch_name}"))?;
                repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
            }
        }

        // Link the branches for future 'Save' calls
        let mut branch = repo.find_branch(&local_branch_name, git2::BranchType::Local)?;
        branch.set_upstream(Some(&format!("{remote_name}/{local_branch_name}")))?;
        println!("--- Tracking relationship established ---");
    } else {
        println!("--- Remote is empty. Ready for your first 'Save'. ---");
    }

    Ok(())
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

pub fn show_progress<F, R>(message: &str, action: F) -> Result<R, Error>
where
    F: FnOnce() -> Result<R, Error>,
{
    print!("{message}... ");
    std::io::stdout().flush().unwrap();
    match action() {
        Ok(result) => {
            println!("{}", "Done".green());
            Ok(result)
        }
        Err(e) => {
            println!("{}", "Error".red());
            Err(e)
        }
    }
}

pub fn get_pr_link(repo: &Repository) -> Option<String> {
    // 1. Get the current branch name (e.g., "feature/my-new-thing")
    let head = if let Ok(head) = repo.head() {
        head
    } else {
        return None;
    };
    let branch_name = head.shorthand().unwrap_or("main");

    // 2. Get the remote URL (usually "origin")
    let remote = if let Ok(remote) = repo.find_remote("origin") {
        remote
    } else {
        return None;
    };
    let remote_url_str = remote.url()?;

    // 3. Parse the URL (handles git@... and https://...)
    let parsed = if let Ok(parsed) = GitUrl::parse(remote_url_str) {
        parsed
    } else {
        return None;
    };

    // 4. Construct the PR URL based on the provider
    // Note: 'parsed.host' returns Option<&str>, usually "github.com", "gitlab.com", etc.
    let host = parsed.host().unwrap_or("");

    let pr_url = match host {
        "github.com" => {
            let provider_info: GenericProvider = if let Ok(info) = parsed.provider_info() {
                info
            } else {
                return None;
            };
            let path = format!("{}/{}", provider_info.owner(), provider_info.repo()); // owner/repo

            // GitHub format: https://github.com/OWNER/REPO/compare/BRANCH?expand=1
            format!("https://github.com/{path}/compare/{branch_name}?expand=1")
        }
        "gitlab.com" => {
            let provider_info: GitLabProvider = if let Ok(info) = parsed.provider_info() {
                info
            } else {
                return None;
            };
            let path = format!("{}/{}", provider_info.owner(), provider_info.repo()); // owner/repo

            // GitLab format: https://gitlab.com/OWNER/REPO/-/merge_requests/new?merge_request[source_branch]=BRANCH
            format!(
                "https://gitlab.com/{path}/-/merge_requests/new?merge_request[source_branch]={branch_name}"
            )
        }
        "bitbucket.org" => {
            let provider_info: AzureDevOpsProvider = if let Ok(info) = parsed.provider_info() {
                info
            } else {
                return None;
            };
            let path = provider_info.fullname(); // org/project/repo

            // Bitbucket format: https://bitbucket.org/OWNER/REPO/pull-requests/new?source=BRANCH
            format!("https://bitbucket.org/{path}/pull-requests/new?source={branch_name}")
        }
        _ => {
            let provider_info: GenericProvider = if let Ok(info) = parsed.provider_info() {
                info
            } else {
                return None;
            };
            let path = format!("{}/{}", provider_info.owner(), provider_info.repo()); // owner/repo

            // Fallback or error for unknown forges
            format!("https://{host}/{path}/pull/new/{branch_name}")
        }
    };

    Some(pr_url)
}
