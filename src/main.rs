mod git_commands;
mod helpers;

use clap::{Parser, Subcommand};
use git2::{BranchType, Error, Repository};

use git_commands::*;
use helpers::*;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path of the repo, defaults to "."
    path: Option<String>,

    /// Turn debugging information on
    #[arg(short, long, action = clap::ArgAction::Count)]
    debug: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Git pull + switch [-c] <name> [+ push]
    Feature {
        #[arg(short, long)]
        name: String,
    },

    /// Git pull + commit + push
    Save {
        #[arg(short, long)]
        message: Option<String>,

        /// Preview the message and changes without committing
        #[arg(short, long, default_value_t = false)]
        dry_run: bool,
    },

    /// Git switch main + git pull [+ git branch -D <branch>]
    Done {
        #[arg(short, long, default_value_t = false)]
        no_clean: bool,
    },

    Creds {
        name: String,
        email: String,

        /// Set settings globally (~/.gitconfig) instead of locally
        #[arg(short, long)]
        global: bool,
    },
    /// Set or update a remote URL (defaults to origin)
    Remote {
        /// The URL of the remote (e.g., git@github.com:user/repo.git)
        url: String,

        /// The name of the remote
        #[arg(short, long, default_value = "origin")]
        name: String,
    },
}

fn main() {
    let cli = Cli::parse();

    match run(cli) {
        Ok(()) => {}
        Err(e) => match e.code() {
            _ => println!("{}", e.message()),
        },
    };
}

fn run(cli: Cli) -> Result<(), Error> {
    let path_str = cli.path.unwrap_or_else(|| String::from("."));
    let repo = Repository::open(&path_str)?;

    match cli.command {
        Commands::Save { .. } | Commands::Creds { .. } => {
            // These commands are allowed to run in a dirty repo
        }
        _ => {
            // All other commands (Feature, Done, Remote) require a clean state
            if is_dirty(&repo)? {
                eprintln!("Error: You have unstaged changes or untracked files.");
                eprintln!("Please 'Save' your work or stash your changes before proceeding.");
                std::process::exit(1);
            }
        }
    };

    match cli.command {
        Commands::Feature { name } => {
            println!("--- Syncing current branch ---");
            pull(&repo, "origin", "HEAD")?;

            println!("--- Switching to feature branch: {} ---", name);
            // Try to find the branch, if not found, create it
            let branch = repo.find_branch(&name, BranchType::Local).or_else(|_| {
                let head = repo.head()?.peel_to_commit()?;
                repo.branch(&name, &head, false)
            });

            // If we still fail (e.g., invalid name), return error
            let branch = branch?;

            // Switch to it (checkout)
            let refname = branch.get().name().unwrap();
            repo.set_head(refname)?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::default().safe()))?;

            println!("--- Pushing upstream ---");
            push(&repo, "origin", &name)?;
        }
        Commands::Save { message, dry_run } => {
            if !dry_run {
                println!("--- Pulling latest changes ---");
                pull(&repo, "origin", "HEAD")?;
            }

            println!("--- Staging and Analyzing ---");
            let msg = match message {
                Some(m) => m,
                None => generate_conventional_message(&repo)?,
            };

            if dry_run {
                println!("\n[DRY RUN] Would have committed with message:");
                println!(">> {}\n", msg);
                println!("To execute, run without the -d flag.");
            } else {
                println!("--- Committing: \"{}\" ---", msg);
                commit_all(&repo, &msg)?;

                println!("--- Pushing ---");
                let head = repo.head()?;
                let branch_name = head.shorthand().unwrap_or("HEAD");
                push(&repo, "origin", branch_name)?;
            }
        }
        Commands::Done { no_clean } => {
            // Identify current branch to delete later
            let head = repo.head()?;
            let current_branch_name = head
                .shorthand()
                .ok_or_else(|| Error::from_str("Not on a valid branch"))?
                .to_string();

            // Determine main branch name (main or master)
            let main_branch = if repo.find_branch("main", BranchType::Local).is_ok() {
                "main"
            } else {
                "master"
            };

            if current_branch_name == main_branch {
                println!("Already on {}, nothing to finalize.", main_branch);
                return Ok(());
            }

            println!("--- Switching to {} ---", main_branch);
            repo.set_head(&format!("refs/heads/{}", main_branch))?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::default().safe()))?;

            println!("--- Pulling {} ---", main_branch);
            pull(&repo, "origin", main_branch)?;

            if !no_clean {
                println!("--- Deleting branch {} ---", current_branch_name);
                let mut branch = repo.find_branch(&current_branch_name, BranchType::Local)?;
                branch.delete()?;
            }
        }
        Commands::Creds {
            name,
            email,
            global,
        } => {
            configure_git(&repo, &name, &email, global)?;
            let scope = if global { "globally" } else { "locally" };
            println!("--- Configured {} as {} <{}> ---", scope, name, email);
        }
        Commands::Remote { url, name } => {
            setup_remote(&repo, &name, &url)?;
            println!("--- Remote '{}' set to {} ---", name, url);

            println!("--- Syncing with remote ---");
            if let Err(e) = sync_remote(&repo, &name) {
                println!("Note: Linked remote, but couldn't sync histories: {}", e);
                println!("You may need to manual merge if histories are unrelated.");
            } else {
                println!(
                    "--- Successfully linked and synced local branch to {}/HEAD ---",
                    name
                );
            }
        }
    };

    Ok(())
}

// --- Helper Functions ---
