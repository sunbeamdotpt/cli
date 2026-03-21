#![cfg(feature = "integration")]
mod helpers;
use helpers::*;
use sunbeam_sdk::client::{AuthMethod, ServiceClient};
use sunbeam_sdk::gitea::GiteaClient;
use sunbeam_sdk::gitea::types::*;
use base64::Engine as _;

const GITEA_API: &str = "http://localhost:3000/api/v1";

fn make_client(pat: &str) -> GiteaClient {
    GiteaClient::from_parts(GITEA_API.to_string(), AuthMethod::Token(pat.to_string()))
}

/// Delete an org via raw API (no SDK method exists).
async fn delete_org(pat: &str, org: &str) {
    let _ = reqwest::Client::new()
        .delete(format!("{GITEA_API}/orgs/{org}"))
        .header("Authorization", format!("token {pat}"))
        .send()
        .await;
}

/// Create a file in a repo via Gitea API (for PR tests that need a commit on a branch).
async fn create_file_on_branch(pat: &str, owner: &str, repo: &str, branch: &str, path: &str) {
    let body = serde_json::json!({
        "content": base64::engine::general_purpose::STANDARD.encode(format!("# {path}\n").as_bytes()),
        "message": format!("add {path}"),
        "branch": branch,
    });
    let resp = reqwest::Client::new()
        .post(format!("{GITEA_API}/repos/{owner}/{repo}/contents/{path}"))
        .header("Authorization", format!("token {pat}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "create file failed: {}",
        resp.text().await.unwrap_or_default()
    );
}

// ---------------------------------------------------------------------------
// 1. User operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_operations() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);

    // get_authenticated_user
    let me = client.get_authenticated_user().await.unwrap();
    assert_eq!(me.login, GITEA_ADMIN_USER);
    assert!(!me.email.is_empty());

    // get_user
    let user = client.get_user(GITEA_ADMIN_USER).await.unwrap();
    assert_eq!(user.login, GITEA_ADMIN_USER);
    assert_eq!(user.id, me.id);

    // search_users
    let result = client.search_users("test", Some(10)).await.unwrap();
    assert!(!result.data.is_empty());
    assert!(result.data.iter().any(|u| u.login == GITEA_ADMIN_USER));
}

// ---------------------------------------------------------------------------
// 2. Repo CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn repo_crud() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("crud-repo");

    // create_user_repo
    let repo = client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            description: Some("integration test".into()),
            private: Some(false),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(repo.name, repo_name);
    assert!(!repo.empty);

    // get_repo
    let fetched = client.get_repo(GITEA_ADMIN_USER, &repo_name).await.unwrap();
    assert_eq!(fetched.id, repo.id);
    assert_eq!(fetched.description, "integration test");

    // edit_repo
    let edited = client
        .edit_repo(
            GITEA_ADMIN_USER,
            &repo_name,
            &EditRepoBody {
                description: Some("updated description".into()),
                has_wiki: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(edited.description, "updated description");

    // search_repos
    let search = client.search_repos(&repo_name, Some(5)).await.unwrap();
    assert!(search.data.iter().any(|r| r.name == repo_name));

    // delete_repo
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();

    // Confirm deletion
    let err = client.get_repo(GITEA_ADMIN_USER, &repo_name).await;
    assert!(err.is_err());
}

// ---------------------------------------------------------------------------
// 3. Repo fork and transfer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn repo_fork_and_transfer() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let src_name = unique_name("fork-src");
    let org_name = unique_name("xfer-org");

    // Create source repo
    client
        .create_user_repo(&CreateRepoBody {
            name: src_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    // Create an org to fork/transfer into
    client
        .create_org(&CreateOrgBody {
            username: org_name.clone(),
            full_name: None,
            description: None,
            visibility: Some("public".into()),
        })
        .await
        .unwrap();

    // Fork into the org with a different name to avoid collision with transfer
    let fork_name = format!("{src_name}-fork");
    let fork = client
        .fork_repo(
            GITEA_ADMIN_USER,
            &src_name,
            &ForkRepoBody {
                organization: Some(org_name.clone()),
                name: Some(fork_name.clone()),
            },
        )
        .await
        .unwrap();
    assert!(fork.fork);
    assert_eq!(fork.name, fork_name);

    // Transfer the original repo to the org
    let transferred = client
        .transfer_repo(
            GITEA_ADMIN_USER,
            &src_name,
            &TransferRepoBody {
                new_owner: org_name.clone(),
                team_ids: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        transferred.full_name,
        format!("{org_name}/{src_name}")
    );

    // Cleanup
    client.delete_repo(&org_name, &src_name).await.unwrap();
    // Delete the fork (same name, same org)
    // Fork already has the same name; the transfer moved the original so the fork
    // may still be under the org. Best-effort cleanup.
    let _ = client
        .delete_repo(&org_name, &format!("{src_name}"))
        .await;
    delete_org(&pat, &org_name).await;
}

// ---------------------------------------------------------------------------
// 4. Org operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn org_operations() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let org_name = unique_name("test-org");

    // create_org
    let org = client
        .create_org(&CreateOrgBody {
            username: org_name.clone(),
            full_name: Some("Test Org".into()),
            description: Some("integration test org".into()),
            visibility: Some("public".into()),
        })
        .await
        .unwrap();
    assert_eq!(org.username, org_name);
    assert_eq!(org.visibility, "public");

    // get_org
    let fetched = client.get_org(&org_name).await.unwrap();
    assert_eq!(fetched.id, org.id);
    assert_eq!(fetched.description, "integration test org");

    // list_user_orgs
    let orgs = client.list_user_orgs(GITEA_ADMIN_USER).await.unwrap();
    assert!(orgs.iter().any(|o| o.username == org_name));

    // create_org_repo
    let repo_name = unique_name("org-repo");
    let repo = client
        .create_org_repo(
            &org_name,
            &CreateRepoBody {
                name: repo_name.clone(),
                auto_init: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(repo.name, repo_name);

    // list_org_repos
    let repos = client.list_org_repos(&org_name, Some(50)).await.unwrap();
    assert!(repos.iter().any(|r| r.name == repo_name));

    // Cleanup
    client.delete_repo(&org_name, &repo_name).await.unwrap();
    delete_org(&pat, &org_name).await;
}

// ---------------------------------------------------------------------------
// 5. Branch operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn branch_operations() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("branch-repo");

    // Create repo with auto_init so it has a main branch
    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            default_branch: Some("main".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // list_branches — should have at least "main"
    let branches = client
        .list_branches(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
    assert!(!branches.is_empty());
    assert!(branches.iter().any(|b| b.name == "main"));

    // create_branch
    let new_branch = client
        .create_branch(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreateBranchBody {
                new_branch_name: "feature-a".into(),
                old_branch_name: Some("main".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(new_branch.name, "feature-a");

    // Verify new branch shows up
    let branches = client
        .list_branches(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
    assert!(branches.iter().any(|b| b.name == "feature-a"));

    // delete_branch
    client
        .delete_branch(GITEA_ADMIN_USER, &repo_name, "feature-a")
        .await
        .unwrap();

    // Confirm deletion
    let branches = client
        .list_branches(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
    assert!(!branches.iter().any(|b| b.name == "feature-a"));

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 6. Issue CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn issue_crud() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("issue-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    // create_issue
    let issue = client
        .create_issue(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreateIssueBody {
                title: "Test issue".into(),
                body: Some("This is a test issue body".into()),
                assignees: None,
                labels: None,
                milestone: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(issue.title, "Test issue");
    assert_eq!(issue.state, "open");
    assert_eq!(issue.number, 1);

    // get_issue
    let fetched = client
        .get_issue(GITEA_ADMIN_USER, &repo_name, issue.number)
        .await
        .unwrap();
    assert_eq!(fetched.number, issue.number);
    assert_eq!(fetched.body.as_deref(), Some("This is a test issue body"));

    // list_issues (open)
    let open = client
        .list_issues(GITEA_ADMIN_USER, &repo_name, "open", Some(10))
        .await
        .unwrap();
    assert!(open.iter().any(|i| i.number == issue.number));

    // edit_issue — close it
    let closed = client
        .edit_issue(
            GITEA_ADMIN_USER,
            &repo_name,
            issue.number,
            &EditIssueBody {
                title: Some("Test issue (closed)".into()),
                state: Some("closed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(closed.state, "closed");
    assert_eq!(closed.title, "Test issue (closed)");

    // list_issues (closed)
    let closed_list = client
        .list_issues(GITEA_ADMIN_USER, &repo_name, "closed", Some(10))
        .await
        .unwrap();
    assert!(closed_list.iter().any(|i| i.number == issue.number));

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 7. Issue comments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn issue_comments() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("comment-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    let issue = client
        .create_issue(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreateIssueBody {
                title: "Comment test".into(),
                body: None,
                assignees: None,
                labels: None,
                milestone: None,
            },
        )
        .await
        .unwrap();

    // create_issue_comment
    let comment = client
        .create_issue_comment(
            GITEA_ADMIN_USER,
            &repo_name,
            issue.number,
            "First comment",
        )
        .await
        .unwrap();
    assert_eq!(comment.body, "First comment");
    assert!(comment.id > 0);

    // Add a second comment
    let comment2 = client
        .create_issue_comment(
            GITEA_ADMIN_USER,
            &repo_name,
            issue.number,
            "Second comment",
        )
        .await
        .unwrap();
    assert_eq!(comment2.body, "Second comment");

    // list_issue_comments
    let comments = client
        .list_issue_comments(GITEA_ADMIN_USER, &repo_name, issue.number)
        .await
        .unwrap();
    assert_eq!(comments.len(), 2);
    assert!(comments.iter().any(|c| c.body == "First comment"));
    assert!(comments.iter().any(|c| c.body == "Second comment"));

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 8. Pull request operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pull_request_operations() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("pr-repo");

    // Create repo with auto_init
    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            default_branch: Some("main".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Create a feature branch
    client
        .create_branch(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreateBranchBody {
                new_branch_name: "feature-pr".into(),
                old_branch_name: Some("main".into()),
            },
        )
        .await
        .unwrap();

    // Create a file on the feature branch so there is a diff for the PR
    create_file_on_branch(
        &pat,
        GITEA_ADMIN_USER,
        &repo_name,
        "feature-pr",
        "new-file.txt",
    )
    .await;

    // create_pull
    let pr = client
        .create_pull(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreatePullBody {
                title: "Test PR".into(),
                head: "feature-pr".into(),
                base: "main".into(),
                body: Some("PR body".into()),
                assignees: None,
                labels: None,
                milestone: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(pr.title, "Test PR");
    assert_eq!(pr.state, "open");
    assert_eq!(pr.number, 1);

    // get_pull
    let fetched = client
        .get_pull(GITEA_ADMIN_USER, &repo_name, pr.number)
        .await
        .unwrap();
    assert_eq!(fetched.number, pr.number);
    assert_eq!(fetched.body.as_deref(), Some("PR body"));
    let head_ref = fetched.head.as_ref().unwrap();
    assert_eq!(head_ref.ref_name, "feature-pr");

    // list_pulls
    let pulls = client
        .list_pulls(GITEA_ADMIN_USER, &repo_name, "open")
        .await
        .unwrap();
    assert!(pulls.iter().any(|p| p.number == pr.number));

    // merge_pull (Gitea may need a moment before the PR is mergeable)
    let mut merged_ok = false;
    for _ in 0..5 {
        match client
            .merge_pull(
                GITEA_ADMIN_USER,
                &repo_name,
                pr.number,
                &MergePullBody {
                    method: "merge".into(),
                    merge_message: Some("merge test PR".into()),
                    delete_branch_after_merge: Some(true),
                },
            )
            .await
        {
            Ok(()) => { merged_ok = true; break; }
            Err(_) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
        }
    }

    // Verify merged (only if merge succeeded)
    let merged = client
        .get_pull(GITEA_ADMIN_USER, &repo_name, pr.number)
        .await
        .unwrap();
    assert!(merged.merged);

    // list_pulls — closed
    let closed_pulls = client
        .list_pulls(GITEA_ADMIN_USER, &repo_name, "closed")
        .await
        .unwrap();
    assert!(closed_pulls.iter().any(|p| p.number == pr.number));

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 9. File content
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_content() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("file-repo");

    // Create repo with auto_init so README.md exists
    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    // get_file_content
    let file = client
        .get_file_content(GITEA_ADMIN_USER, &repo_name, "README.md", None)
        .await
        .unwrap();
    assert_eq!(file.name, "README.md");
    assert_eq!(file.path, "README.md");
    assert_eq!(file.r#type, "file");
    assert!(file.size > 0);
    assert!(file.content.is_some());
    assert!(!file.sha.is_empty());

    // get_raw_file
    let raw = client
        .get_raw_file(GITEA_ADMIN_USER, &repo_name, "README.md", None)
        .await
        .unwrap();
    assert!(!raw.is_empty());
    let text = String::from_utf8_lossy(&raw);
    assert!(text.contains(&repo_name));

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 10. Notifications
// ---------------------------------------------------------------------------

#[tokio::test]
async fn notifications() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);

    // list_notifications — should succeed, likely empty
    let notifs = client.list_notifications().await.unwrap();
    // No assertion on length; new test user has no notifications.
    // Just verify it returns a valid vec.
    let _ = notifs;

    // mark_notifications_read — should succeed even with nothing to mark
    client.mark_notifications_read().await.unwrap();

    // Verify still returns successfully after marking
    let notifs_after = client.list_notifications().await.unwrap();
    assert!(notifs_after.iter().all(|n| !n.unread));
}

// ---------------------------------------------------------------------------
// 11. File content with ref parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_content_with_ref() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("fileref-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            default_branch: Some("main".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Create a branch and add a file on it
    client
        .create_branch(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreateBranchBody {
                new_branch_name: "ref-branch".into(),
                old_branch_name: Some("main".into()),
            },
        )
        .await
        .unwrap();

    create_file_on_branch(&pat, GITEA_ADMIN_USER, &repo_name, "ref-branch", "branch-file.txt")
        .await;

    // get_file_content with explicit ref
    let file = client
        .get_file_content(GITEA_ADMIN_USER, &repo_name, "branch-file.txt", Some("ref-branch"))
        .await
        .unwrap();
    assert_eq!(file.name, "branch-file.txt");
    assert_eq!(file.r#type, "file");

    // get_file_content should fail on main (file doesn't exist there)
    let err = client
        .get_file_content(GITEA_ADMIN_USER, &repo_name, "branch-file.txt", Some("main"))
        .await;
    assert!(err.is_err());

    // get_raw_file with explicit ref
    let raw = client
        .get_raw_file(GITEA_ADMIN_USER, &repo_name, "branch-file.txt", Some("ref-branch"))
        .await
        .unwrap();
    assert!(!raw.is_empty());

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 12. Mirror sync (on a non-mirror repo — exercises the endpoint, expects error)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mirror_sync_non_mirror() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("mirror-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    // mirror_sync on a non-mirror repo should fail
    let result = client
        .mirror_sync(GITEA_ADMIN_USER, &repo_name)
        .await;
    assert!(result.is_err(), "mirror_sync should fail on non-mirror repo");

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 13. Transfer repo error path (transfer to nonexistent owner)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn transfer_repo_error() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("xfer-err-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    // Transfer to a nonexistent owner should fail
    let result = client
        .transfer_repo(
            GITEA_ADMIN_USER,
            &repo_name,
            &TransferRepoBody {
                new_owner: "nonexistent-owner-zzz".into(),
                team_ids: None,
            },
        )
        .await;
    assert!(result.is_err(), "transfer to nonexistent owner should fail");

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 14. Search users with default limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_users_default_limit() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);

    // search_users with None limit (exercises default limit path)
    let result = client.search_users(GITEA_ADMIN_USER, None).await.unwrap();
    assert!(!result.data.is_empty());
}

// ---------------------------------------------------------------------------
// 15. Search repos with default limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_repos_default_limit() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);

    let result = client.search_repos("", None).await.unwrap();
    // Just verify it returns successfully
    let _ = result.data;
}

// ---------------------------------------------------------------------------
// 16. List issues with default limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_issues_default_limit() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("defissue-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    // list_issues with None limit (exercises default limit path)
    let issues = client
        .list_issues(GITEA_ADMIN_USER, &repo_name, "open", None)
        .await
        .unwrap();
    assert!(issues.is_empty());

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 17. List org repos with default limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_org_repos_default_limit() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let org_name = unique_name("deforg");

    client
        .create_org(&CreateOrgBody {
            username: org_name.clone(),
            full_name: None,
            description: None,
            visibility: Some("public".into()),
        })
        .await
        .unwrap();

    // list_org_repos with None limit
    let repos = client.list_org_repos(&org_name, None).await.unwrap();
    assert!(repos.is_empty());

    // Cleanup
    delete_org(&pat, &org_name).await;
}

// ---------------------------------------------------------------------------
// 18. Get repo error (nonexistent repo)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_repo_nonexistent() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);

    let result = client
        .get_repo(GITEA_ADMIN_USER, "totally-nonexistent-repo-zzz")
        .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// 19. Create branch without old_branch_name (default branch)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_branch_default_base() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("brdefault-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            default_branch: Some("main".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // create_branch without specifying old_branch_name
    let branch = client
        .create_branch(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreateBranchBody {
                new_branch_name: "from-default".into(),
                old_branch_name: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(branch.name, "from-default");

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 20. Edit issue body only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_issue_body() {
    let pat = setup_gitea_pat().await;
    let client = make_client(&pat);
    let repo_name = unique_name("editbody-repo");

    client
        .create_user_repo(&CreateRepoBody {
            name: repo_name.clone(),
            auto_init: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();

    let issue = client
        .create_issue(
            GITEA_ADMIN_USER,
            &repo_name,
            &CreateIssueBody {
                title: "Body edit test".into(),
                body: Some("original body".into()),
                assignees: None,
                labels: None,
                milestone: None,
            },
        )
        .await
        .unwrap();

    // Edit only the body, not the state
    let edited = client
        .edit_issue(
            GITEA_ADMIN_USER,
            &repo_name,
            issue.number,
            &EditIssueBody {
                body: Some("updated body".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(edited.body.as_deref(), Some("updated body"));
    assert_eq!(edited.state, "open"); // state unchanged

    // Cleanup
    client
        .delete_repo(GITEA_ADMIN_USER, &repo_name)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 21. with_token constructor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn with_token_constructor() {
    let pat = setup_gitea_pat().await;
    // Use with_token to construct a client pointing at localhost
    let client = GiteaClient::with_token("localhost:3000", pat);
    // The base_url should be https://src.localhost:3000/api/v1 which won't work,
    // but we can verify the constructor itself sets the URL format.
    assert!(client.base_url().contains("src.localhost"));
}
