mod git_commands;
mod helpers;

use clap::{Parser, Subcommand};
use git2::{Error, Repository};
use owo_colors::OwoColorize;
use std::io::Write;

use git_commands::*;
use helpers::*;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path of the repo, defaults to "."
    #[arg(short, long, default_value = ".")]
    path: String,

    /// Turn debugging information on
    #[arg(short, long, action = clap::ArgAction::Count)]
    debug: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Push {},
    Pull {},
    /// Git pull + switch [-c] <name> [+ push]
    Feature {
        name: String,

        #[arg(short, long)]
        base: Option<String>,
    },

    /// List all branches
    Features {},

    /// Git pull + commit + push
    Save {
        #[arg(short, long)]
        message: Option<String>,
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
    /// Find and manage conflicts create by pulls
    Resolve {
        /// Once you have manually merged the .theirs files, this flag will delete them
        #[arg(long, default_value_t = false)]
        cleanup: bool,
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
    let path_str = cli.path;
    let repo = Repository::open(&path_str)?;

    match cli.command {
        Commands::Feature { .. }
        | Commands::Features { .. }
        | Commands::Save { .. }
        | Commands::Creds { .. }
        | Commands::Resolve { .. } => {
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
        Commands::Push {} => {
            print!("Pushing... ");
            let head = repo.head()?;
            let branch_name = head.shorthand().unwrap_or("HEAD");
            push(&repo, "origin", branch_name)?;
            println!("{}", "Done".green());
        }
        Commands::Pull {} => {
            print!("Pulling... ");
            pull(&repo, "origin", "HEAD")?;
            println!("{}", "Done".green());
        }
        Commands::Features {} => {
            let branches = repo.branches(Some(git2::BranchType::Local))?;
            for b in branches {
                let (branch, _) = b?;
                println!("{}", branch.name()?.unwrap_or("HEAD"));
            }
        }
        Commands::Feature { name, base } => {
            create_feature_branch(&repo, &name, base)?;
        }
        Commands::Save { message } => {
            show_progress("Pulling", || pull(&repo, "origin", "HEAD"))?;

            let msg = show_progress("Staging and Analyzing", || {
                let mut index = repo.index()?;
                index.add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)?;
                index.write()?;

                match message {
                    Some(m) => Ok(m),
                    None => generate_conventional_message(&repo),
                }
            })?;

            show_progress("Committing", || commit_all(&repo, &msg))?;

            show_progress("Pushing", || {
                let head = repo.head()?;
                let branch_name = head.shorthand().unwrap_or("HEAD");
                push(&repo, "origin", branch_name)
            })?;
        }
        Commands::Done { no_clean } => {
            done(&repo, no_clean)?;
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
            // 1. Set or Update URL
            match repo.find_remote(&name) {
                Ok(_) => repo.remote_set_url(&name, &url)?,
                Err(_) => {
                    repo.remote(&name, &url)?;
                }
            }
            println!("--- Remote '{}' set to {} ---", name, url);

            // 2. Perform the "weird shit" sync automatically
            println!("--- Syncing with remote ---");
            if let Err(e) = sync_unrelated_histories(&repo, &name) {
                eprintln!("--- Sync Note: {} ---", e);
                // We don't exit(1) here because the remote URL is still set successfully
            } else {
                println!("--- Pushing ---");
                let head = repo.head()?;
                let branch_name = head.shorthand().unwrap_or("HEAD");
                push(&repo, "origin", branch_name)?;
            }
        }
        Commands::Resolve { cleanup } => {
            resolve(&repo, cleanup)?;
        }
    };

    Ok(())
}

// --- Helper Functions ---
fn show_progress<F, R>(message: &str, action: F) -> Result<R, Error>
where
    F: FnOnce() -> Result<R, Error>,
{
    print!("{}... ", message);
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
