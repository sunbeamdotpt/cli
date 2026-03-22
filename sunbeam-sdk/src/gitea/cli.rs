//! Gitea VCS CLI — `vcs repo|issue|pr|branch|org|user|file|notification`.

use clap::Subcommand;

use crate::client::SunbeamClient;
use crate::error::{Result, SunbeamError};
use crate::gitea::types::*;
use crate::output::{render, render_list, read_json_input, OutputFormat};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn split_repo(repo: &str) -> Result<(&str, &str)> {
    repo.split_once('/')
        .ok_or_else(|| SunbeamError::Other("--repo must be owner/repo".into()))
}

// ---------------------------------------------------------------------------
// Command tree
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum VcsCommand {
    /// Repository operations.
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },
    /// Issue operations.
    Issue {
        #[command(subcommand)]
        action: IssueAction,
    },
    /// Pull request operations.
    Pr {
        #[command(subcommand)]
        action: PrAction,
    },
    /// Branch operations.
    Branch {
        #[command(subcommand)]
        action: BranchAction,
    },
    /// Organization operations.
    Org {
        #[command(subcommand)]
        action: OrgAction,
    },
    /// User operations.
    User {
        #[command(subcommand)]
        action: UserAction,
    },
    /// File operations.
    File {
        #[command(subcommand)]
        action: FileAction,
    },
    /// Notification operations.
    Notification {
        #[command(subcommand)]
        action: NotificationAction,
    },
}

// -- Repo -------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum RepoAction {
    /// List repositories for an organization.
    List {
        /// Organization name.
        #[arg(long)]
        org: String,
        /// Max results.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Search repositories.
    Search {
        /// Search query.
        #[arg(short, long)]
        query: String,
        /// Max results.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get a repository.
    Get {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
    },
    /// Create a repository.
    Create {
        /// Organization (creates org repo). Omit for user repo.
        #[arg(long)]
        org: Option<String>,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
    /// Update a repository.
    Update {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
    /// Delete a repository.
    Delete {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
    },
    /// Fork a repository.
    Fork {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
    /// Trigger mirror sync.
    MirrorSync {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
    },
    /// Transfer a repository to another owner.
    Transfer {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
}

// -- Issue ------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum IssueAction {
    /// List issues.
    List {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Filter by state (open, closed, all).
        #[arg(long, default_value = "open")]
        state: String,
        /// Max results.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get an issue.
    Get {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Issue number.
        #[arg(short, long)]
        id: u64,
    },
    /// Create an issue.
    Create {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
    /// Update an issue.
    Update {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Issue number.
        #[arg(short, long)]
        id: u64,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
    /// Add a comment to an issue.
    Comment {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Issue number.
        #[arg(short, long)]
        id: u64,
        /// Comment body text.
        #[arg(long)]
        body: String,
    },
}

// -- PR ---------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum PrAction {
    /// List pull requests.
    List {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Filter by state (open, closed, all).
        #[arg(long, default_value = "open")]
        state: String,
    },
    /// Get a pull request.
    Get {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Pull request number.
        #[arg(short, long)]
        id: u64,
    },
    /// Create a pull request.
    Create {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
    /// Merge a pull request.
    Merge {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Pull request number.
        #[arg(short, long)]
        id: u64,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
}

// -- Branch -----------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum BranchAction {
    /// List branches.
    List {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
    },
    /// Create a branch.
    Create {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
    /// Delete a branch.
    Delete {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// Branch name.
        #[arg(long)]
        branch: String,
    },
}

// -- Org --------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum OrgAction {
    /// List organizations for a user.
    List {
        /// Username.
        #[arg(long)]
        user: String,
    },
    /// Get an organization.
    Get {
        /// Organization name.
        #[arg(long)]
        org: String,
    },
    /// Create an organization.
    Create {
        /// JSON body or "-" for stdin.
        #[arg(long)]
        data: Option<String>,
    },
}

// -- User -------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum UserAction {
    /// Search users.
    Search {
        /// Search query.
        #[arg(short, long)]
        query: String,
        /// Max results.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get a user by username.
    Get {
        /// Username.
        #[arg(long)]
        user: String,
    },
    /// Get the authenticated user.
    Me,
}

// -- File -------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum FileAction {
    /// Get file content.
    Get {
        /// Repository (owner/repo).
        #[arg(short, long)]
        repo: String,
        /// File path within the repository.
        #[arg(long)]
        path: String,
        /// Git ref (branch, tag, commit SHA).
        #[arg(long, name = "ref")]
        git_ref: Option<String>,
    },
}

// -- Notification -----------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum NotificationAction {
    /// List notifications.
    List,
    /// Mark all notifications as read.
    Read,
}

// ---------------------------------------------------------------------------
// Table row helpers
// ---------------------------------------------------------------------------

fn repo_row(r: &Repository) -> Vec<String> {
    vec![
        r.full_name.clone(),
        if r.private { "private" } else { "public" }.into(),
        r.stars_count.to_string(),
        r.forks_count.to_string(),
        r.default_branch.clone(),
    ]
}

fn issue_row(i: &Issue) -> Vec<String> {
    vec![
        format!("#{}", i.number),
        i.title.clone(),
        i.state.clone(),
        i.created_at.clone().unwrap_or_default(),
    ]
}

fn pr_row(pr: &PullRequest) -> Vec<String> {
    vec![
        format!("#{}", pr.number),
        pr.title.clone(),
        pr.state.clone(),
        if pr.merged { "yes" } else { "no" }.into(),
    ]
}

fn branch_row(b: &Branch) -> Vec<String> {
    vec![
        b.name.clone(),
        if b.protected { "yes" } else { "no" }.into(),
        b.commit
            .as_ref()
            .map(|c| c.id.chars().take(8).collect::<String>())
            .unwrap_or_default(),
    ]
}

fn org_row(o: &Organization) -> Vec<String> {
    vec![
        o.username.clone(),
        o.full_name.clone(),
        o.visibility.clone(),
    ]
}

fn user_row(u: &User) -> Vec<String> {
    vec![
        u.login.clone(),
        u.full_name.clone(),
        u.email.clone(),
        if u.is_admin { "admin" } else { "" }.into(),
    ]
}

fn notification_row(n: &Notification) -> Vec<String> {
    let subject_title = n
        .subject
        .as_ref()
        .map(|s| s.title.clone())
        .unwrap_or_default();
    let repo_name = n
        .repository
        .as_ref()
        .map(|r| r.full_name.clone())
        .unwrap_or_default();
    vec![
        n.id.to_string(),
        repo_name,
        subject_title,
        if n.unread { "unread" } else { "read" }.into(),
    ]
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn dispatch(cmd: VcsCommand, client: &SunbeamClient, fmt: OutputFormat) -> Result<()> {
    let client = client.gitea().await?;
    match cmd {
        // -- Repo -----------------------------------------------------------
        VcsCommand::Repo { action } => match action {
            RepoAction::List { org, limit } => {
                let repos = client.list_org_repos(&org, limit).await?;
                render_list(
                    &repos,
                    &["NAME", "VISIBILITY", "STARS", "FORKS", "BRANCH"],
                    repo_row,
                    fmt,
                )
            }
            RepoAction::Search { query, limit } => {
                let result = client.search_repos(&query, limit).await?;
                render_list(
                    &result.data,
                    &["NAME", "VISIBILITY", "STARS", "FORKS", "BRANCH"],
                    repo_row,
                    fmt,
                )
            }
            RepoAction::Get { repo } => {
                let (owner, name) = split_repo(&repo)?;
                let r = client.get_repo(owner, name).await?;
                render(&r, fmt)
            }
            RepoAction::Create { org, data } => {
                let val = read_json_input(data.as_deref())?;
                let body: CreateRepoBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid repo body: {e}")))?;
                let r = match org {
                    Some(org) => client.create_org_repo(&org, &body).await?,
                    None => client.create_user_repo(&body).await?,
                };
                render(&r, fmt)
            }
            RepoAction::Update { repo, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = read_json_input(data.as_deref())?;
                let body: EditRepoBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid repo body: {e}")))?;
                let r = client.edit_repo(owner, name, &body).await?;
                render(&r, fmt)
            }
            RepoAction::Delete { repo } => {
                let (owner, name) = split_repo(&repo)?;
                client.delete_repo(owner, name).await?;
                crate::output::ok(&format!("Deleted repository {repo}"));
                Ok(())
            }
            RepoAction::Fork { repo, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = data
                    .as_deref()
                    .map(|d| read_json_input(Some(d)))
                    .transpose()?
                    .unwrap_or_else(|| serde_json::json!({}));
                let body: ForkRepoBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid fork body: {e}")))?;
                let r = client.fork_repo(owner, name, &body).await?;
                render(&r, fmt)
            }
            RepoAction::MirrorSync { repo } => {
                let (owner, name) = split_repo(&repo)?;
                client.mirror_sync(owner, name).await?;
                crate::output::ok(&format!("Mirror sync triggered for {repo}"));
                Ok(())
            }
            RepoAction::Transfer { repo, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = read_json_input(data.as_deref())?;
                let body: TransferRepoBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid transfer body: {e}")))?;
                let r = client.transfer_repo(owner, name, &body).await?;
                render(&r, fmt)
            }
        },

        // -- Issue ----------------------------------------------------------
        VcsCommand::Issue { action } => match action {
            IssueAction::List { repo, state, limit } => {
                let (owner, name) = split_repo(&repo)?;
                let issues = client.list_issues(owner, name, &state, limit).await?;
                render_list(
                    &issues,
                    &["#", "TITLE", "STATE", "CREATED"],
                    issue_row,
                    fmt,
                )
            }
            IssueAction::Get { repo, id } => {
                let (owner, name) = split_repo(&repo)?;
                let issue = client.get_issue(owner, name, id).await?;
                render(&issue, fmt)
            }
            IssueAction::Create { repo, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = read_json_input(data.as_deref())?;
                let body: CreateIssueBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid issue body: {e}")))?;
                let issue = client.create_issue(owner, name, &body).await?;
                render(&issue, fmt)
            }
            IssueAction::Update { repo, id, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = read_json_input(data.as_deref())?;
                let body: EditIssueBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid issue body: {e}")))?;
                let issue = client.edit_issue(owner, name, id, &body).await?;
                render(&issue, fmt)
            }
            IssueAction::Comment { repo, id, body } => {
                let (owner, name) = split_repo(&repo)?;
                let comment = client.create_issue_comment(owner, name, id, &body).await?;
                render(&comment, fmt)
            }
        },

        // -- PR -------------------------------------------------------------
        VcsCommand::Pr { action } => match action {
            PrAction::List { repo, state } => {
                let (owner, name) = split_repo(&repo)?;
                let prs = client.list_pulls(owner, name, &state).await?;
                render_list(
                    &prs,
                    &["#", "TITLE", "STATE", "MERGED"],
                    pr_row,
                    fmt,
                )
            }
            PrAction::Get { repo, id } => {
                let (owner, name) = split_repo(&repo)?;
                let pr = client.get_pull(owner, name, id).await?;
                render(&pr, fmt)
            }
            PrAction::Create { repo, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = read_json_input(data.as_deref())?;
                let body: CreatePullBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid PR body: {e}")))?;
                let pr = client.create_pull(owner, name, &body).await?;
                render(&pr, fmt)
            }
            PrAction::Merge { repo, id, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = data
                    .as_deref()
                    .map(|d| read_json_input(Some(d)))
                    .transpose()?
                    .unwrap_or_else(|| serde_json::json!({"Do": "merge"}));
                let body: MergePullBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid merge body: {e}")))?;
                client.merge_pull(owner, name, id, &body).await?;
                crate::output::ok(&format!("Merged PR #{id} in {repo}"));
                Ok(())
            }
        },

        // -- Branch ---------------------------------------------------------
        VcsCommand::Branch { action } => match action {
            BranchAction::List { repo } => {
                let (owner, name) = split_repo(&repo)?;
                let branches = client.list_branches(owner, name).await?;
                render_list(
                    &branches,
                    &["NAME", "PROTECTED", "COMMIT"],
                    branch_row,
                    fmt,
                )
            }
            BranchAction::Create { repo, data } => {
                let (owner, name) = split_repo(&repo)?;
                let val = read_json_input(data.as_deref())?;
                let body: CreateBranchBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid branch body: {e}")))?;
                let branch = client.create_branch(owner, name, &body).await?;
                render(&branch, fmt)
            }
            BranchAction::Delete { repo, branch } => {
                let (owner, name) = split_repo(&repo)?;
                client.delete_branch(owner, name, &branch).await?;
                crate::output::ok(&format!("Deleted branch {branch} from {repo}"));
                Ok(())
            }
        },

        // -- Org ------------------------------------------------------------
        VcsCommand::Org { action } => match action {
            OrgAction::List { user } => {
                let orgs = client.list_user_orgs(&user).await?;
                render_list(
                    &orgs,
                    &["USERNAME", "FULL NAME", "VISIBILITY"],
                    org_row,
                    fmt,
                )
            }
            OrgAction::Get { org } => {
                let o = client.get_org(&org).await?;
                render(&o, fmt)
            }
            OrgAction::Create { data } => {
                let val = read_json_input(data.as_deref())?;
                let body: CreateOrgBody = serde_json::from_value(val)
                    .map_err(|e| SunbeamError::Other(format!("invalid org body: {e}")))?;
                let o = client.create_org(&body).await?;
                render(&o, fmt)
            }
        },

        // -- User -----------------------------------------------------------
        VcsCommand::User { action } => match action {
            UserAction::Search { query, limit } => {
                let result = client.search_users(&query, limit).await?;
                render_list(
                    &result.data,
                    &["LOGIN", "NAME", "EMAIL", "ROLE"],
                    user_row,
                    fmt,
                )
            }
            UserAction::Get { user } => {
                let u = client.get_user(&user).await?;
                render(&u, fmt)
            }
            UserAction::Me => {
                let u = client.get_authenticated_user().await?;
                render(&u, fmt)
            }
        },

        // -- File -----------------------------------------------------------
        VcsCommand::File { action } => match action {
            FileAction::Get { repo, path, git_ref } => {
                let (owner, name) = split_repo(&repo)?;
                let fc = client
                    .get_file_content(owner, name, &path, git_ref.as_deref())
                    .await?;
                render(&fc, fmt)
            }
        },

        // -- Notification ---------------------------------------------------
        VcsCommand::Notification { action } => match action {
            NotificationAction::List => {
                let notes = client.list_notifications().await?;
                render_list(
                    &notes,
                    &["ID", "REPO", "SUBJECT", "STATUS"],
                    notification_row,
                    fmt,
                )
            }
            NotificationAction::Read => {
                client.mark_notifications_read().await?;
                crate::output::ok("All notifications marked as read");
                Ok(())
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_repo_valid() {
        let (owner, repo) = split_repo("studio/cli").unwrap();
        assert_eq!(owner, "studio");
        assert_eq!(repo, "cli");
    }

    #[test]
    fn test_split_repo_invalid() {
        assert!(split_repo("noslash").is_err());
    }

    #[test]
    fn test_split_repo_nested() {
        // Only splits on first slash
        let (owner, repo) = split_repo("org/repo/extra").unwrap();
        assert_eq!(owner, "org");
        assert_eq!(repo, "repo/extra");
    }

    #[test]
    fn test_repo_row() {
        let r = Repository {
            full_name: "studio/cli".into(),
            private: true,
            stars_count: 5,
            forks_count: 2,
            default_branch: "main".into(),
            ..Default::default()
        };
        let row = repo_row(&r);
        assert_eq!(row[0], "studio/cli");
        assert_eq!(row[1], "private");
        assert_eq!(row[2], "5");
        assert_eq!(row[3], "2");
        assert_eq!(row[4], "main");
    }

    #[test]
    fn test_repo_row_public() {
        let r = Repository {
            full_name: "studio/web".into(),
            private: false,
            ..Default::default()
        };
        assert_eq!(repo_row(&r)[1], "public");
    }

    #[test]
    fn test_issue_row() {
        let i = Issue {
            number: 42,
            title: "Fix bug".into(),
            state: "open".into(),
            created_at: Some("2026-01-15".into()),
            body: None,
            assignees: None,
            labels: None,
            updated_at: None,
            html_url: None,
            repository: None,
            milestone: None,
            comments: None,
        };
        let row = issue_row(&i);
        assert_eq!(row[0], "#42");
        assert_eq!(row[1], "Fix bug");
        assert_eq!(row[2], "open");
        assert_eq!(row[3], "2026-01-15");
    }

    #[test]
    fn test_pr_row_merged() {
        let pr = PullRequest {
            number: 7,
            title: "Add feature".into(),
            state: "closed".into(),
            merged: true,
            body: None,
            head: None,
            base: None,
            mergeable: None,
            html_url: None,
            assignees: None,
            labels: None,
            created_at: None,
            updated_at: None,
        };
        let row = pr_row(&pr);
        assert_eq!(row[0], "#7");
        assert_eq!(row[3], "yes");
    }

    #[test]
    fn test_pr_row_not_merged() {
        let pr = PullRequest {
            number: 3,
            title: "WIP".into(),
            state: "open".into(),
            merged: false,
            body: None,
            head: None,
            base: None,
            mergeable: None,
            html_url: None,
            assignees: None,
            labels: None,
            created_at: None,
            updated_at: None,
        };
        assert_eq!(pr_row(&pr)[3], "no");
    }

    #[test]
    fn test_branch_row() {
        let b = Branch {
            name: "main".into(),
            protected: true,
            commit: Some(BranchCommit {
                id: "abcdef1234567890".into(),
                message: "init".into(),
            }),
        };
        let row = branch_row(&b);
        assert_eq!(row[0], "main");
        assert_eq!(row[1], "yes");
        assert_eq!(row[2], "abcdef12");
    }

    #[test]
    fn test_branch_row_no_commit() {
        let b = Branch {
            name: "feature".into(),
            protected: false,
            commit: None,
        };
        let row = branch_row(&b);
        assert_eq!(row[1], "no");
        assert_eq!(row[2], "");
    }

    #[test]
    fn test_org_row() {
        let o = Organization {
            id: 1,
            username: "studio".into(),
            full_name: "Studio Org".into(),
            visibility: "public".into(),
            description: String::new(),
            avatar_url: String::new(),
        };
        let row = org_row(&o);
        assert_eq!(row[0], "studio");
        assert_eq!(row[1], "Studio Org");
        assert_eq!(row[2], "public");
    }

    #[test]
    fn test_user_row_admin() {
        let u = User {
            id: 1,
            login: "alice".into(),
            full_name: "Alice".into(),
            email: "alice@example.com".into(),
            is_admin: true,
            avatar_url: String::new(),
        };
        let row = user_row(&u);
        assert_eq!(row[0], "alice");
        assert_eq!(row[3], "admin");
    }

    #[test]
    fn test_user_row_regular() {
        let u = User {
            login: "bob".into(),
            is_admin: false,
            ..Default::default()
        };
        assert_eq!(user_row(&u)[3], "");
    }

    #[test]
    fn test_notification_row() {
        let n = Notification {
            id: 99,
            subject: Some(NotificationSubject {
                title: "New issue".into(),
                url: String::new(),
                r#type: "Issue".into(),
                state: None,
            }),
            repository: Some(Repository {
                full_name: "studio/cli".into(),
                ..Default::default()
            }),
            unread: true,
            updated_at: None,
        };
        let row = notification_row(&n);
        assert_eq!(row[0], "99");
        assert_eq!(row[1], "studio/cli");
        assert_eq!(row[2], "New issue");
        assert_eq!(row[3], "unread");
    }

    #[test]
    fn test_notification_row_read() {
        let n = Notification {
            id: 1,
            subject: None,
            repository: None,
            unread: false,
            updated_at: None,
        };
        let row = notification_row(&n);
        assert_eq!(row[3], "read");
        // Missing subject/repo should produce empty strings
        assert_eq!(row[1], "");
        assert_eq!(row[2], "");
    }
}
