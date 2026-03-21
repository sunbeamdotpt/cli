//! Gitea API types.

use serde::{Deserialize, Serialize};

/// Generic search result wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult<T> {
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub data: Vec<T>,
}

/// A Gitea repository.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Repository {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub clone_url: String,
    #[serde(default)]
    pub ssh_url: String,
    #[serde(default)]
    pub default_branch: String,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub mirror: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub empty: bool,
    #[serde(default)]
    pub stars_count: u64,
    #[serde(default)]
    pub forks_count: u64,
    #[serde(default)]
    pub open_issues_count: u64,
    #[serde(default)]
    pub owner: Option<User>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Body for creating a repository.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateRepoBody {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_init: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gitignores: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
}

/// Body for editing a repository.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditRepoBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_issues: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_pull_requests: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_wiki: Option<bool>,
}

/// Body for forking a repository.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForkRepoBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
}

/// Body for transferring a repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRepoBody {
    pub new_owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_ids: Option<Vec<u64>>,
}

/// A Gitea issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub assignees: Option<Vec<User>>,
    #[serde(default)]
    pub labels: Option<Vec<Label>>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub repository: Option<RepositoryMeta>,
    #[serde(default)]
    pub milestone: Option<Milestone>,
    #[serde(default)]
    pub comments: Option<u64>,
}

/// Body for creating an issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIssueBody {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<u64>,
}

/// Body for editing an issue.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditIssueBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<String>>,
}

/// A pull request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub head: Option<PullRequestRef>,
    #[serde(default)]
    pub base: Option<PullRequestRef>,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub mergeable: Option<bool>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub assignees: Option<Vec<User>>,
    #[serde(default)]
    pub labels: Option<Vec<Label>>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A pull request branch ref.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestRef {
    #[serde(default)]
    pub label: String,
    #[serde(rename = "ref", default)]
    pub ref_name: String,
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub repo: Option<Repository>,
}

/// Body for creating a pull request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePullBody {
    pub title: String,
    pub head: String,
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<u64>,
}

/// Body for merging a pull request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MergePullBody {
    #[serde(rename = "Do")]
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "merge_message_field")]
    pub merge_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_branch_after_merge: Option<bool>,
}

/// A comment on an issue or pull request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A Gitea user.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct User {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub is_admin: bool,
}

/// A label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub description: String,
}

/// A branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub commit: Option<BranchCommit>,
    #[serde(default)]
    pub protected: bool,
}

/// A commit on a branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchCommit {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub message: String,
}

/// Body for creating a branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBranchBody {
    pub new_branch_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_branch_name: Option<String>,
}

/// An organization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub id: u64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub visibility: String,
}

/// Body for creating an organization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOrgBody {
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

/// A milestone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub open_issues: u64,
    #[serde(default)]
    pub closed_issues: u64,
}

/// Repository metadata (minimal, for issue responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryMeta {
    #[serde(default)]
    pub full_name: Option<String>,
}

/// File content response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub download_url: Option<String>,
}

/// A notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    #[serde(default)]
    pub subject: Option<NotificationSubject>,
    #[serde(default)]
    pub repository: Option<Repository>,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Notification subject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSubject {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_roundtrip() {
        let json = serde_json::json!({
            "id": 1,
            "name": "cli",
            "full_name": "studio/cli",
            "default_branch": "main"
        });
        let repo: Repository = serde_json::from_value(json).unwrap();
        assert_eq!(repo.name, "cli");
        assert_eq!(repo.full_name, "studio/cli");
    }

    #[test]
    fn test_issue_roundtrip() {
        let json = serde_json::json!({
            "number": 42,
            "title": "Bug report",
            "state": "open"
        });
        let issue: Issue = serde_json::from_value(json).unwrap();
        assert_eq!(issue.number, 42);
        assert_eq!(issue.state, "open");
    }

    #[test]
    fn test_create_repo_body() {
        let body = CreateRepoBody {
            name: "new-repo".into(),
            description: Some("A test repo".into()),
            private: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["name"], "new-repo");
        assert_eq!(json["private"], true);
        assert!(json.get("auto_init").is_none());
    }

    #[test]
    fn test_search_result() {
        let json = serde_json::json!({
            "ok": true,
            "data": [{"id": 1, "login": "alice", "full_name": "Alice", "email": "", "avatar_url": "", "is_admin": false}]
        });
        let result: SearchResult<User> = serde_json::from_value(json).unwrap();
        assert_eq!(result.data.len(), 1);
        assert_eq!(result.data[0].login, "alice");
    }
}
