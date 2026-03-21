//! Gitea issues client.

use serde::{Deserialize, Serialize};

use crate::error::{Result, SunbeamError};
use super::{Ticket, Source, Status};

// ---------------------------------------------------------------------------
// IssueUpdate
// ---------------------------------------------------------------------------

/// Update payload for a Gitea issue.
#[derive(Debug, Default, Serialize)]
pub struct IssueUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

// ---------------------------------------------------------------------------
// GiteaClient
// ---------------------------------------------------------------------------

pub(super) struct GiteaClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

/// Serde helpers for Gitea JSON responses.
pub(super) mod gitea_json {
    use super::*;

    #[derive(Debug, Deserialize)]
    pub struct Issue {
        pub number: u64,
        #[serde(default)]
        pub title: String,
        #[serde(default)]
        pub body: Option<String>,
        #[serde(default)]
        pub state: String,
        #[serde(default)]
        pub assignees: Option<Vec<GiteaUser>>,
        #[serde(default)]
        pub labels: Option<Vec<GiteaLabel>>,
        #[serde(default)]
        pub created_at: Option<String>,
        #[serde(default)]
        pub updated_at: Option<String>,
        #[serde(default)]
        pub html_url: Option<String>,
        #[serde(default)]
        pub repository: Option<Repository>,
    }

    #[derive(Debug, Deserialize)]
    pub struct GiteaUser {
        #[serde(default)]
        pub login: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct GiteaLabel {
        #[serde(default)]
        pub name: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct Repository {
        #[serde(default)]
        pub full_name: Option<String>,
    }

    impl Issue {
        pub fn to_ticket(self, base_url: &str, org: &str, repo: &str) -> Ticket {
            let status = state_to_status(&self.state);
            let assignees = self
                .assignees
                .unwrap_or_default()
                .into_iter()
                .map(|u| u.login)
                .collect();
            let labels = self
                .labels
                .unwrap_or_default()
                .into_iter()
                .map(|l| l.name)
                .collect();
            let web_base = base_url.trim_end_matches("/api/v1");
            let url = self
                .html_url
                .unwrap_or_else(|| format!("{web_base}/{org}/{repo}/issues/{}", self.number));

            Ticket {
                id: format!("g:{org}/{repo}#{}", self.number),
                source: Source::Gitea,
                title: self.title,
                description: self.body.unwrap_or_default(),
                status,
                assignees,
                labels,
                created_at: self.created_at.unwrap_or_default(),
                updated_at: self.updated_at.unwrap_or_default(),
                url,
            }
        }
    }

    /// Map Gitea issue state to normalised status.
    pub fn state_to_status(state: &str) -> Status {
        match state {
            "open" => Status::Open,
            "closed" => Status::Closed,
            _ => Status::Open,
        }
    }
}

impl GiteaClient {
    /// Create a new Gitea client using the Hydra OAuth2 token.
    pub(super) async fn new(domain: &str) -> Result<Self> {
        let base_url = format!("https://src.{domain}/api/v1");
        // Gitea needs its own PAT, not the Hydra access token
        let token = crate::auth::get_gitea_token()?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| SunbeamError::network(format!("Failed to build HTTP client: {e}")))?;

        Ok(Self {
            base_url,
            token,
            http,
        })
    }

    /// Build a request with Gitea PAT auth (`Authorization: token <pat>`).
    #[allow(dead_code)]
    fn authed_get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header("Authorization", format!("token {}", self.token))
    }

    #[allow(dead_code)]
    fn authed_post(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .post(url)
            .header("Authorization", format!("token {}", self.token))
    }

    #[allow(dead_code)]
    fn authed_patch(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .patch(url)
            .header("Authorization", format!("token {}", self.token))
    }

    /// List issues for an org/repo (or search across an org).
    pub(super) fn list_issues<'a>(
        &'a self,
        org: &'a str,
        repo: Option<&'a str>,
        state: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<Vec<Ticket>>> {
        Box::pin(self.list_issues_inner(org, repo, state))
    }

    async fn list_issues_inner(
        &self,
        org: &str,
        repo: Option<&str>,
        state: &str,
    ) -> Result<Vec<Ticket>> {
        match repo {
            Some(r) => {
                let url = format!("{}/repos/{org}/{r}/issues", self.base_url);
                let resp = self
                    .http
                    .get(&url)
                    .header("Authorization", format!("token {}", self.token))
                    .query(&[("state", state), ("type", "issues"), ("limit", "50")])
                    .send()
                    .await
                    .map_err(|e| SunbeamError::network(format!("Gitea list_issues: {e}")))?;

                if !resp.status().is_success() {
                    return Err(SunbeamError::network(format!(
                        "Gitea GET issues for {org}/{r} returned {}",
                        resp.status()
                    )));
                }

                let issues: Vec<gitea_json::Issue> = resp
                    .json()
                    .await
                    .map_err(|e| SunbeamError::network(format!("Gitea issues parse error: {e}")))?;

                Ok(issues
                    .into_iter()
                    .map(|i| i.to_ticket(&self.base_url, org, r))
                    .collect())
            }
            None => {
                // Search across the entire org by listing org repos, then issues.
                let repos_url = format!("{}/orgs/{org}/repos", self.base_url);
                let repos_resp = self
                    .http
                    .get(&repos_url)
                    .header("Authorization", format!("token {}", self.token))
                    .query(&[("limit", "50")])
                    .send()
                    .await
                    .map_err(|e| SunbeamError::network(format!("Gitea list org repos: {e}")))?;

                if !repos_resp.status().is_success() {
                    return Err(SunbeamError::network(format!(
                        "Gitea GET repos for org {org} returned {}",
                        repos_resp.status()
                    )));
                }

                #[derive(Deserialize)]
                struct Repo {
                    name: String,
                }
                let repos: Vec<Repo> = repos_resp
                    .json()
                    .await
                    .map_err(|e| SunbeamError::network(format!("Gitea repos parse: {e}")))?;

                let mut all = Vec::new();
                for r in &repos {
                    match self.list_issues(org, Some(&r.name), state).await {
                        Ok(mut tickets) => all.append(&mut tickets),
                        Err(_) => continue, // skip repos we cannot read
                    }
                }
                Ok(all)
            }
        }
    }

    /// GET /api/v1/repos/{org}/{repo}/issues/{index}
    pub(super) async fn get_issue(&self, org: &str, repo: &str, index: u64) -> Result<Ticket> {
        let url = format!("{}/repos/{org}/{repo}/issues/{index}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea get_issue: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Gitea GET issue {org}/{repo}#{index} returned {}",
                resp.status()
            )));
        }

        let issue: gitea_json::Issue = resp
            .json()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea issue parse: {e}")))?;

        Ok(issue.to_ticket(&self.base_url, org, repo))
    }

    /// POST /api/v1/repos/{org}/{repo}/issues
    pub(super) async fn create_issue(
        &self,
        org: &str,
        repo: &str,
        title: &str,
        body: &str,
    ) -> Result<Ticket> {
        let url = format!("{}/repos/{org}/{repo}/issues", self.base_url);
        let payload = serde_json::json!({
            "title": title,
            "body": body,
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea create_issue: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Gitea POST issue to {org}/{repo} returned {}",
                resp.status()
            )));
        }

        let issue: gitea_json::Issue = resp
            .json()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea issue create parse: {e}")))?;

        Ok(issue.to_ticket(&self.base_url, org, repo))
    }

    /// PATCH /api/v1/repos/{org}/{repo}/issues/{index}
    pub(super) async fn update_issue(
        &self,
        org: &str,
        repo: &str,
        index: u64,
        updates: &IssueUpdate,
    ) -> Result<()> {
        let url = format!("{}/repos/{org}/{repo}/issues/{index}", self.base_url);
        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(updates)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea update_issue: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Gitea PATCH issue {org}/{repo}#{index} returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// Close an issue.
    pub(super) async fn close_issue(&self, org: &str, repo: &str, index: u64) -> Result<()> {
        self.update_issue(
            org,
            repo,
            index,
            &IssueUpdate {
                state: Some("closed".to_string()),
                ..Default::default()
            },
        )
        .await
    }

    /// POST /api/v1/repos/{org}/{repo}/issues/{index}/comments
    pub(super) async fn comment_issue(
        &self,
        org: &str,
        repo: &str,
        index: u64,
        body: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{org}/{repo}/issues/{index}/comments",
            self.base_url
        );
        let payload = serde_json::json!({ "body": body });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea comment_issue: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Gitea POST comment on {org}/{repo}#{index} returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// POST /api/v1/repos/{org}/{repo}/issues/{index}/assignees
    #[allow(dead_code)]
    pub(super) async fn assign_issue(
        &self,
        org: &str,
        repo: &str,
        index: u64,
        assignee: &str,
    ) -> Result<()> {
        // Use PATCH on the issue itself -- the /assignees endpoint requires
        // the user to be an explicit collaborator, while PATCH works for
        // any org member with write access.
        let url = format!(
            "{}/repos/{org}/{repo}/issues/{index}",
            self.base_url
        );
        let payload = serde_json::json!({ "assignees": [assignee] });

        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(&payload)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea assign_issue: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Gitea assign on {org}/{repo}#{index} returned {}",
                resp.status()
            )));
        }
        Ok(())
    }
}
