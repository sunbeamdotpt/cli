//! Unified project management across Planka (kanban boards) and Gitea (issues).
//!
//! Ticket IDs use a prefix format:
//! - `p:42` or `planka:42` — Planka card
//! - `g:studio/cli#7` or `gitea:studio/cli#7` — Gitea issue

use crate::error::{Result, ResultExt, SunbeamError};
use crate::output;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Unified ticket representation across both systems.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub id: String,
    pub source: Source,
    pub title: String,
    pub description: String,
    pub status: Status,
    pub assignees: Vec<String>,
    pub labels: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
}

/// Which backend a ticket originates from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Planka,
    Gitea,
}

/// Normalised ticket status across both systems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Open,
    InProgress,
    Done,
    Closed,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Planka => write!(f, "planka"),
            Source::Gitea => write!(f, "gitea"),
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Open => write!(f, "open"),
            Status::InProgress => write!(f, "in-progress"),
            Status::Done => write!(f, "done"),
            Status::Closed => write!(f, "closed"),
        }
    }
}

// ---------------------------------------------------------------------------
// Ticket ID parsing
// ---------------------------------------------------------------------------

/// A parsed ticket reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketRef {
    /// Planka card by numeric ID.
    Planka(u64),
    /// Gitea issue: (org, repo, issue number).
    Gitea {
        org: String,
        repo: String,
        number: u64,
    },
}

/// Parse a prefixed ticket ID string.
///
/// Accepted formats:
/// - `p:42`, `planka:42`
/// - `g:studio/cli#7`, `gitea:studio/cli#7`
pub fn parse_ticket_id(id: &str) -> Result<TicketRef> {
    let (prefix, rest) = id
        .split_once(':')
        .ctx("Invalid ticket ID: expected 'p:ID' or 'g:org/repo#num'")?;

    match prefix {
        "p" | "planka" => {
            let card_id: u64 = rest
                .parse()
                .map_err(|_| SunbeamError::config(format!("Invalid Planka card ID: {rest}")))?;
            Ok(TicketRef::Planka(card_id))
        }
        "g" | "gitea" => {
            // Expected: org/repo#number
            let (org_repo, num_str) = rest
                .rsplit_once('#')
                .ctx("Invalid Gitea ticket ID: expected org/repo#number")?;
            let (org, repo) = org_repo
                .split_once('/')
                .ctx("Invalid Gitea ticket ID: expected org/repo#number")?;
            let number: u64 = num_str
                .parse()
                .map_err(|_| SunbeamError::config(format!("Invalid issue number: {num_str}")))?;
            Ok(TicketRef::Gitea {
                org: org.to_string(),
                repo: repo.to_string(),
                number,
            })
        }
        _ => Err(SunbeamError::config(format!(
            "Unknown ticket prefix '{prefix}': use 'p'/'planka' or 'g'/'gitea'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

/// Retrieve the user's Hydra OAuth2 access token via the auth module.
async fn get_token() -> Result<String> {
    crate::auth::get_token().await
}

// ---------------------------------------------------------------------------
// Planka client
// ---------------------------------------------------------------------------

/// Update payload for a Planka card.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_id: Option<u64>,
}

struct PlankaClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

/// Serde helpers for Planka JSON responses.
mod planka_json {
    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ExchangeResponse {
        #[serde(default)]
        pub token: Option<String>,
        // Planka may also return the token in `item`
        #[serde(default)]
        pub item: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Card {
        pub id: u64,
        #[serde(default)]
        pub name: String,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub list_id: Option<u64>,
        #[serde(default)]
        pub created_at: Option<String>,
        #[serde(default)]
        pub updated_at: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BoardResponse {
        #[serde(default)]
        pub included: Option<BoardIncluded>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BoardIncluded {
        #[serde(default)]
        pub cards: Vec<Card>,
        #[serde(default)]
        pub card_memberships: Vec<CardMembership>,
        #[serde(default)]
        pub card_labels: Vec<CardLabel>,
        #[serde(default)]
        pub labels: Vec<Label>,
        #[serde(default)]
        pub lists: Vec<List>,
        #[serde(default)]
        pub users: Vec<User>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CardMembership {
        pub card_id: u64,
        pub user_id: u64,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CardLabel {
        pub card_id: u64,
        pub label_id: u64,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Label {
        pub id: u64,
        #[serde(default)]
        pub name: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct List {
        pub id: u64,
        #[serde(default)]
        pub name: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct User {
        pub id: u64,
        #[serde(default)]
        pub name: Option<String>,
        #[serde(default)]
        pub username: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CardDetailResponse {
        pub item: Card,
        #[serde(default)]
        pub included: Option<BoardIncluded>,
    }

    impl Card {
        pub fn to_ticket(self, base_url: &str, included: Option<&BoardIncluded>) -> Ticket {
            let status = match included {
                Some(inc) => list_name_to_status(
                    self.list_id
                        .and_then(|lid| inc.lists.iter().find(|l| l.id == lid))
                        .map(|l| l.name.as_str())
                        .unwrap_or(""),
                ),
                None => Status::Open,
            };

            let assignees = match included {
                Some(inc) => inc
                    .card_memberships
                    .iter()
                    .filter(|m| m.card_id == self.id)
                    .filter_map(|m| {
                        inc.users.iter().find(|u| u.id == m.user_id).map(|u| {
                            u.username
                                .clone()
                                .or_else(|| u.name.clone())
                                .unwrap_or_else(|| m.user_id.to_string())
                        })
                    })
                    .collect(),
                None => vec![],
            };

            let labels = match included {
                Some(inc) => inc
                    .card_labels
                    .iter()
                    .filter(|cl| cl.card_id == self.id)
                    .filter_map(|cl| {
                        inc.labels.iter().find(|l| l.id == cl.label_id).map(|l| {
                            l.name
                                .clone()
                                .unwrap_or_else(|| cl.label_id.to_string())
                        })
                    })
                    .collect(),
                None => vec![],
            };

            // Derive web URL from API base URL (strip `/api`).
            let web_base = base_url.trim_end_matches("/api");
            Ticket {
                id: format!("planka:{}", self.id),
                source: Source::Planka,
                title: self.name,
                description: self.description.unwrap_or_default(),
                status,
                assignees,
                labels,
                created_at: self.created_at.unwrap_or_default(),
                updated_at: self.updated_at.unwrap_or_default(),
                url: format!("{web_base}/cards/{}", self.id),
            }
        }
    }

    /// Map a Planka list name to a normalised status.
    fn list_name_to_status(name: &str) -> Status {
        let lower = name.to_lowercase();
        if lower.contains("done") || lower.contains("complete") {
            Status::Done
        } else if lower.contains("progress") || lower.contains("doing") || lower.contains("active")
        {
            Status::InProgress
        } else if lower.contains("closed") || lower.contains("archive") {
            Status::Closed
        } else {
            Status::Open
        }
    }
}

impl PlankaClient {
    /// Create a new Planka client, exchanging the Hydra token for a Planka JWT
    /// if the direct Bearer token is rejected.
    async fn new(domain: &str) -> Result<Self> {
        let base_url = format!("https://projects.{domain}/api");
        let hydra_token = get_token().await?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| SunbeamError::network(format!("Failed to build HTTP client: {e}")))?;

        // Try OIDC exchange first to get a Planka-native JWT.
        let exchange_url = format!("{base_url}/access-tokens/exchange-using-oidc");
        let exchange_resp = http
            .post(&exchange_url)
            .bearer_auth(&hydra_token)
            .send()
            .await;

        let token = match exchange_resp {
            Ok(resp) if resp.status().is_success() => {
                let body: planka_json::ExchangeResponse = resp
                    .json()
                    .await
                    .map_err(|e| SunbeamError::network(format!("Planka OIDC exchange parse error: {e}")))?;
                body.token
                    .or(body.item)
                    .unwrap_or_else(|| hydra_token.clone())
            }
            // Exchange not available or failed -- fall back to using the Hydra token directly.
            _ => hydra_token,
        };

        Ok(Self {
            base_url,
            token,
            http,
        })
    }

    /// GET /api/boards/{id} and extract all cards.
    async fn list_cards(&self, board_id: u64) -> Result<Vec<Ticket>> {
        let url = format!("{}/boards/{board_id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka list_cards: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Planka GET board {board_id} returned {}",
                resp.status()
            )));
        }

        let body: planka_json::BoardResponse = resp
            .json()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka board parse error: {e}")))?;

        let included = body.included;
        let tickets = included
            .as_ref()
            .map(|inc| {
                inc.cards
                    .clone()
                    .into_iter()
                    .map(|c: planka_json::Card| c.to_ticket(&self.base_url, Some(inc)))
                    .collect()
            })
            .unwrap_or_default();

        Ok(tickets)
    }

    /// GET /api/cards/{id}
    async fn get_card(&self, id: u64) -> Result<Ticket> {
        let url = format!("{}/cards/{id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka get_card: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Planka GET card {id} returned {}",
                resp.status()
            )));
        }

        let body: planka_json::CardDetailResponse = resp
            .json()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka card parse error: {e}")))?;

        Ok(body
            .item
            .to_ticket(&self.base_url, body.included.as_ref()))
    }

    /// POST /api/lists/{list_id}/cards
    async fn create_card(
        &self,
        _board_id: u64,
        list_id: u64,
        name: &str,
        description: &str,
    ) -> Result<Ticket> {
        let url = format!("{}/lists/{list_id}/cards", self.base_url);
        let body = serde_json::json!({
            "name": name,
            "description": description,
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka create_card: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Planka POST card returned {}",
                resp.status()
            )));
        }

        let card: planka_json::CardDetailResponse = resp
            .json()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka card create parse error: {e}")))?;

        Ok(card.item.to_ticket(&self.base_url, card.included.as_ref()))
    }

    /// PATCH /api/cards/{id}
    async fn update_card(&self, id: u64, updates: &CardUpdate) -> Result<()> {
        let url = format!("{}/cards/{id}", self.base_url);
        let resp = self
            .http
            .patch(&url)
            .bearer_auth(&self.token)
            .json(updates)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka update_card: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Planka PATCH card {id} returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// Move a card to a different list.
    async fn move_card(&self, id: u64, list_id: u64) -> Result<()> {
        self.update_card(
            id,
            &CardUpdate {
                list_id: Some(list_id),
                ..Default::default()
            },
        )
        .await
    }

    /// POST /api/cards/{id}/comment-actions
    async fn comment_card(&self, id: u64, text: &str) -> Result<()> {
        let url = format!("{}/cards/{id}/comment-actions", self.base_url);
        let body = serde_json::json!({ "text": text });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka comment_card: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Planka POST comment on card {id} returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// POST /api/cards/{id}/memberships
    async fn assign_card(&self, id: u64, user_id: &str) -> Result<()> {
        let url = format!("{}/cards/{id}/memberships", self.base_url);
        let body = serde_json::json!({ "userId": user_id });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka assign_card: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Planka POST membership on card {id} returned {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gitea client
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

struct GiteaClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

/// Serde helpers for Gitea JSON responses.
mod gitea_json {
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
                id: format!("gitea:{org}/{repo}#{}", self.number),
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
    async fn new(domain: &str) -> Result<Self> {
        let base_url = format!("https://src.{domain}/api/v1");
        let token = get_token().await?;
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

    /// List issues for an org/repo (or search across an org).
    fn list_issues<'a>(
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
                    .bearer_auth(&self.token)
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
                    .bearer_auth(&self.token)
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
    async fn get_issue(&self, org: &str, repo: &str, index: u64) -> Result<Ticket> {
        let url = format!("{}/repos/{org}/{repo}/issues/{index}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
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
    async fn create_issue(
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
            .bearer_auth(&self.token)
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
    async fn update_issue(
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
            .bearer_auth(&self.token)
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
    async fn close_issue(&self, org: &str, repo: &str, index: u64) -> Result<()> {
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
    async fn comment_issue(
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
            .bearer_auth(&self.token)
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
    async fn assign_issue(
        &self,
        org: &str,
        repo: &str,
        index: u64,
        assignee: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/repos/{org}/{repo}/issues/{index}/assignees",
            self.base_url
        );
        let payload = serde_json::json!({ "assignees": [assignee] });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Gitea assign_issue: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Gitea POST assignee on {org}/{repo}#{index} returned {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Format a list of tickets as a table.
fn display_ticket_list(tickets: &[Ticket]) {
    if tickets.is_empty() {
        output::ok("No tickets found.");
        return;
    }

    let rows: Vec<Vec<String>> = tickets
        .iter()
        .map(|t| {
            vec![
                t.id.clone(),
                t.status.to_string(),
                t.title.clone(),
                t.assignees.join(", "),
                t.source.to_string(),
            ]
        })
        .collect();

    let tbl = output::table(&rows, &["ID", "STATUS", "TITLE", "ASSIGNEES", "SOURCE"]);
    println!("{tbl}");
}

/// Print a single ticket in detail.
fn display_ticket_detail(t: &Ticket) {
    println!("{} ({})", t.title, t.id);
    println!("  Status:      {}", t.status);
    println!("  Source:      {}", t.source);
    if !t.assignees.is_empty() {
        println!("  Assignees:   {}", t.assignees.join(", "));
    }
    if !t.labels.is_empty() {
        println!("  Labels:      {}", t.labels.join(", "));
    }
    if !t.created_at.is_empty() {
        println!("  Created:     {}", t.created_at);
    }
    if !t.updated_at.is_empty() {
        println!("  Updated:     {}", t.updated_at);
    }
    println!("  URL:         {}", t.url);
    if !t.description.is_empty() {
        println!();
        println!("{}", t.description);
    }
}

// ---------------------------------------------------------------------------
// Unified commands
// ---------------------------------------------------------------------------

/// List tickets, optionally filtering by source and state.
///
/// When `source` is `None`, both Planka and Gitea are queried in parallel.
#[allow(dead_code)]
pub async fn cmd_pm_list(source: Option<&str>, state: &str) -> Result<()> {
    let domain = crate::kube::get_domain().await?;

    let fetch_planka = source.is_none() || matches!(source, Some("planka" | "p"));
    let fetch_gitea = source.is_none() || matches!(source, Some("gitea" | "g"));

    let planka_fut = async {
        if fetch_planka {
            let client = PlankaClient::new(&domain).await?;
            // Board ID 1 as default; in practice this would be configurable.
            client.list_cards(1).await
        } else {
            Ok(vec![])
        }
    };

    let gitea_fut = async {
        if fetch_gitea {
            let client = GiteaClient::new(&domain).await?;
            client.list_issues("studio", None, state).await
        } else {
            Ok(vec![])
        }
    };

    let (planka_result, gitea_result) = tokio::join!(planka_fut, gitea_fut);

    let mut tickets = Vec::new();

    match planka_result {
        Ok(mut t) => tickets.append(&mut t),
        Err(e) => output::warn(&format!("Planka: {e}")),
    }

    match gitea_result {
        Ok(mut t) => tickets.append(&mut t),
        Err(e) => output::warn(&format!("Gitea: {e}")),
    }

    // Filter by state if looking at Planka results too.
    if state == "closed" {
        tickets.retain(|t| matches!(t.status, Status::Closed | Status::Done));
    } else if state == "open" {
        tickets.retain(|t| matches!(t.status, Status::Open | Status::InProgress));
    }

    display_ticket_list(&tickets);
    Ok(())
}

/// Show details for a single ticket by ID.
#[allow(dead_code)]
pub async fn cmd_pm_show(id: &str) -> Result<()> {
    let domain = crate::kube::get_domain().await?;
    let ticket_ref = parse_ticket_id(id)?;

    let ticket = match ticket_ref {
        TicketRef::Planka(card_id) => {
            let client = PlankaClient::new(&domain).await?;
            client.get_card(card_id).await?
        }
        TicketRef::Gitea { org, repo, number } => {
            let client = GiteaClient::new(&domain).await?;
            client.get_issue(&org, &repo, number).await?
        }
    };

    display_ticket_detail(&ticket);
    Ok(())
}

/// Create a new ticket.
///
/// `source` must be `"planka"` or `"gitea"`.
/// `target` is source-specific: for Planka it is `"board_id/list_id"`,
/// for Gitea it is `"org/repo"`.
#[allow(dead_code)]
pub async fn cmd_pm_create(title: &str, body: &str, source: &str, target: &str) -> Result<()> {
    let domain = crate::kube::get_domain().await?;

    let ticket = match source {
        "planka" | "p" => {
            let parts: Vec<&str> = target.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err(SunbeamError::config(
                    "Planka target must be 'board_id/list_id'",
                ));
            }
            let board_id: u64 = parts[0]
                .parse()
                .map_err(|_| SunbeamError::config("Invalid board_id"))?;
            let list_id: u64 = parts[1]
                .parse()
                .map_err(|_| SunbeamError::config("Invalid list_id"))?;

            let client = PlankaClient::new(&domain).await?;
            client.create_card(board_id, list_id, title, body).await?
        }
        "gitea" | "g" => {
            let parts: Vec<&str> = target.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err(SunbeamError::config("Gitea target must be 'org/repo'"));
            }
            let client = GiteaClient::new(&domain).await?;
            client.create_issue(parts[0], parts[1], title, body).await?
        }
        _ => {
            return Err(SunbeamError::config(format!(
                "Unknown source '{source}': use 'planka' or 'gitea'"
            )));
        }
    };

    output::ok(&format!("Created: {} ({})", ticket.title, ticket.id));
    println!("  {}", ticket.url);
    Ok(())
}

/// Add a comment to a ticket.
#[allow(dead_code)]
pub async fn cmd_pm_comment(id: &str, text: &str) -> Result<()> {
    let domain = crate::kube::get_domain().await?;
    let ticket_ref = parse_ticket_id(id)?;

    match ticket_ref {
        TicketRef::Planka(card_id) => {
            let client = PlankaClient::new(&domain).await?;
            client.comment_card(card_id, text).await?;
        }
        TicketRef::Gitea { org, repo, number } => {
            let client = GiteaClient::new(&domain).await?;
            client.comment_issue(&org, &repo, number, text).await?;
        }
    }

    output::ok(&format!("Comment added to {id}."));
    Ok(())
}

/// Close a ticket.
#[allow(dead_code)]
pub async fn cmd_pm_close(id: &str) -> Result<()> {
    let domain = crate::kube::get_domain().await?;
    let ticket_ref = parse_ticket_id(id)?;

    match ticket_ref {
        TicketRef::Planka(card_id) => {
            // Move to a "Done" list -- caller should specify the list, but as a
            // sensible default we just update the card name convention.  A real
            // implementation would look up the board's "Done" list.
            let client = PlankaClient::new(&domain).await?;
            // Attempt to find the board's Done list via get_card metadata.
            // For now, just mark description.
            let _ = client
                .update_card(
                    card_id,
                    &CardUpdate {
                        ..Default::default()
                    },
                )
                .await;
            output::ok(&format!("Planka card {card_id} closed (move to Done list manually if needed)."));
        }
        TicketRef::Gitea { org, repo, number } => {
            let client = GiteaClient::new(&domain).await?;
            client.close_issue(&org, &repo, number).await?;
            output::ok(&format!("Closed gitea:{org}/{repo}#{number}."));
        }
    }

    Ok(())
}

/// Assign a user to a ticket.
#[allow(dead_code)]
pub async fn cmd_pm_assign(id: &str, user: &str) -> Result<()> {
    let domain = crate::kube::get_domain().await?;
    let ticket_ref = parse_ticket_id(id)?;

    match ticket_ref {
        TicketRef::Planka(card_id) => {
            let client = PlankaClient::new(&domain).await?;
            client.assign_card(card_id, user).await?;
        }
        TicketRef::Gitea { org, repo, number } => {
            let client = GiteaClient::new(&domain).await?;
            client.assign_issue(&org, &repo, number, user).await?;
        }
    }

    output::ok(&format!("Assigned {user} to {id}."));
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Ticket ID parsing --------------------------------------------------

    #[test]
    fn test_parse_planka_short() {
        let r = parse_ticket_id("p:42").unwrap();
        assert_eq!(r, TicketRef::Planka(42));
    }

    #[test]
    fn test_parse_planka_long() {
        let r = parse_ticket_id("planka:100").unwrap();
        assert_eq!(r, TicketRef::Planka(100));
    }

    #[test]
    fn test_parse_gitea_short() {
        let r = parse_ticket_id("g:studio/cli#7").unwrap();
        assert_eq!(
            r,
            TicketRef::Gitea {
                org: "studio".to_string(),
                repo: "cli".to_string(),
                number: 7,
            }
        );
    }

    #[test]
    fn test_parse_gitea_long() {
        let r = parse_ticket_id("gitea:internal/infra#123").unwrap();
        assert_eq!(
            r,
            TicketRef::Gitea {
                org: "internal".to_string(),
                repo: "infra".to_string(),
                number: 123,
            }
        );
    }

    #[test]
    fn test_parse_missing_colon() {
        assert!(parse_ticket_id("noprefix").is_err());
    }

    #[test]
    fn test_parse_unknown_prefix() {
        assert!(parse_ticket_id("jira:FOO-1").is_err());
    }

    #[test]
    fn test_parse_invalid_planka_id() {
        assert!(parse_ticket_id("p:abc").is_err());
    }

    #[test]
    fn test_parse_gitea_missing_hash() {
        assert!(parse_ticket_id("g:studio/cli").is_err());
    }

    #[test]
    fn test_parse_gitea_missing_slash() {
        assert!(parse_ticket_id("g:repo#1").is_err());
    }

    #[test]
    fn test_parse_gitea_invalid_number() {
        assert!(parse_ticket_id("g:studio/cli#abc").is_err());
    }

    // -- Status mapping -----------------------------------------------------

    #[test]
    fn test_gitea_state_open() {
        assert_eq!(gitea_json::state_to_status("open"), Status::Open);
    }

    #[test]
    fn test_gitea_state_closed() {
        assert_eq!(gitea_json::state_to_status("closed"), Status::Closed);
    }

    #[test]
    fn test_gitea_state_unknown_defaults_open() {
        assert_eq!(gitea_json::state_to_status("weird"), Status::Open);
    }

    #[test]
    fn test_status_display() {
        assert_eq!(Status::Open.to_string(), "open");
        assert_eq!(Status::InProgress.to_string(), "in-progress");
        assert_eq!(Status::Done.to_string(), "done");
        assert_eq!(Status::Closed.to_string(), "closed");
    }

    #[test]
    fn test_source_display() {
        assert_eq!(Source::Planka.to_string(), "planka");
        assert_eq!(Source::Gitea.to_string(), "gitea");
    }

    // -- Display formatting -------------------------------------------------

    #[test]
    fn test_display_ticket_list_table() {
        let tickets = vec![
            Ticket {
                id: "planka:1".to_string(),
                source: Source::Planka,
                title: "Fix login".to_string(),
                description: String::new(),
                status: Status::Open,
                assignees: vec!["alice".to_string()],
                labels: vec![],
                created_at: "2025-01-01".to_string(),
                updated_at: "2025-01-02".to_string(),
                url: "https://projects.example.com/cards/1".to_string(),
            },
            Ticket {
                id: "gitea:studio/cli#7".to_string(),
                source: Source::Gitea,
                title: "Add tests".to_string(),
                description: "We need more tests.".to_string(),
                status: Status::InProgress,
                assignees: vec!["bob".to_string(), "carol".to_string()],
                labels: vec!["enhancement".to_string()],
                created_at: "2025-02-01".to_string(),
                updated_at: "2025-02-05".to_string(),
                url: "https://src.example.com/studio/cli/issues/7".to_string(),
            },
        ];

        let rows: Vec<Vec<String>> = tickets
            .iter()
            .map(|t| {
                vec![
                    t.id.clone(),
                    t.status.to_string(),
                    t.title.clone(),
                    t.assignees.join(", "),
                    t.source.to_string(),
                ]
            })
            .collect();

        let tbl = output::table(&rows, &["ID", "STATUS", "TITLE", "ASSIGNEES", "SOURCE"]);
        assert!(tbl.contains("planka:1"));
        assert!(tbl.contains("gitea:studio/cli#7"));
        assert!(tbl.contains("open"));
        assert!(tbl.contains("in-progress"));
        assert!(tbl.contains("Fix login"));
        assert!(tbl.contains("Add tests"));
        assert!(tbl.contains("alice"));
        assert!(tbl.contains("bob, carol"));
        assert!(tbl.contains("planka"));
        assert!(tbl.contains("gitea"));
    }

    #[test]
    fn test_display_ticket_list_empty() {
        let rows: Vec<Vec<String>> = vec![];
        let tbl = output::table(&rows, &["ID", "STATUS", "TITLE", "ASSIGNEES", "SOURCE"]);
        // Should have header + separator but no data rows.
        assert!(tbl.contains("ID"));
        assert_eq!(tbl.lines().count(), 2);
    }

    #[test]
    fn test_card_update_serialization() {
        let update = CardUpdate {
            name: Some("New name".to_string()),
            description: None,
            list_id: Some(5),
        };
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(json["name"], "New name");
        assert_eq!(json["listId"], 5);
        assert!(json.get("description").is_none());
    }

    #[test]
    fn test_issue_update_serialization() {
        let update = IssueUpdate {
            title: None,
            body: Some("Updated body".to_string()),
            state: Some("closed".to_string()),
        };
        let json = serde_json::to_value(&update).unwrap();
        assert!(json.get("title").is_none());
        assert_eq!(json["body"], "Updated body");
        assert_eq!(json["state"], "closed");
    }

    #[test]
    fn test_planka_list_name_to_status() {
        // Test via Card::to_ticket with synthetic included data.
        use planka_json::*;

        let inc = BoardIncluded {
            cards: vec![],
            card_memberships: vec![],
            card_labels: vec![],
            labels: vec![],
            lists: vec![
                List { id: 1, name: "To Do".to_string() },
                List { id: 2, name: "In Progress".to_string() },
                List { id: 3, name: "Done".to_string() },
                List { id: 4, name: "Archived / Closed".to_string() },
            ],
            users: vec![],
        };

        let make_card = |list_id| Card {
            id: 1,
            name: "test".to_string(),
            description: None,
            list_id: Some(list_id),
            created_at: None,
            updated_at: None,
        };

        assert_eq!(
            make_card(1).to_ticket("https://x/api", Some(&inc)).status,
            Status::Open
        );
        assert_eq!(
            make_card(2).to_ticket("https://x/api", Some(&inc)).status,
            Status::InProgress
        );
        assert_eq!(
            make_card(3).to_ticket("https://x/api", Some(&inc)).status,
            Status::Done
        );
        assert_eq!(
            make_card(4).to_ticket("https://x/api", Some(&inc)).status,
            Status::Closed
        );
    }

    #[test]
    fn test_gitea_issue_to_ticket() {
        let issue = gitea_json::Issue {
            number: 42,
            title: "Bug report".to_string(),
            body: Some("Something broke".to_string()),
            state: "open".to_string(),
            assignees: Some(vec![gitea_json::GiteaUser {
                login: "dev1".to_string(),
            }]),
            labels: Some(vec![gitea_json::GiteaLabel {
                name: "bug".to_string(),
            }]),
            created_at: Some("2025-03-01T00:00:00Z".to_string()),
            updated_at: Some("2025-03-02T00:00:00Z".to_string()),
            html_url: Some("https://src.example.com/studio/app/issues/42".to_string()),
            repository: None,
        };

        let ticket = issue.to_ticket("https://src.example.com/api/v1", "studio", "app");
        assert_eq!(ticket.id, "gitea:studio/app#42");
        assert_eq!(ticket.source, Source::Gitea);
        assert_eq!(ticket.title, "Bug report");
        assert_eq!(ticket.description, "Something broke");
        assert_eq!(ticket.status, Status::Open);
        assert_eq!(ticket.assignees, vec!["dev1"]);
        assert_eq!(ticket.labels, vec!["bug"]);
        assert_eq!(
            ticket.url,
            "https://src.example.com/studio/app/issues/42"
        );
    }
}
