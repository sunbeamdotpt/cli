//! Planka (kanban board) client.

use serde::Serialize;

use crate::error::{Result, SunbeamError};
use super::{get_token, Ticket, Source, Status};

// ---------------------------------------------------------------------------
// CardUpdate
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
    pub list_id: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// PlankaClient
// ---------------------------------------------------------------------------

pub(super) struct PlankaClient {
    pub(super) base_url: String,
    pub(super) token: String,
    pub(super) http: reqwest::Client,
}

/// Serde helpers for Planka JSON responses.
pub(super) mod planka_json {
    use super::*;
    use serde::Deserialize;

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
        pub id: serde_json::Value,
        #[serde(default)]
        pub name: String,
        #[serde(default)]
        pub description: Option<String>,
        #[serde(default)]
        pub list_id: Option<serde_json::Value>,
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
        pub card_id: serde_json::Value,
        pub user_id: serde_json::Value,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CardLabel {
        pub card_id: serde_json::Value,
        pub label_id: serde_json::Value,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Label {
        pub id: serde_json::Value,
        #[serde(default)]
        pub name: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct List {
        pub id: serde_json::Value,
        #[serde(default)]
        pub name: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct User {
        pub id: serde_json::Value,
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
                id: format!("p:{}", self.id.as_str().unwrap_or(&self.id.to_string())),
                source: Source::Planka,
                title: self.name,
                description: self.description.unwrap_or_default(),
                status,
                assignees,
                labels,
                created_at: self.created_at.unwrap_or_default(),
                updated_at: self.updated_at.unwrap_or_default(),
                url: format!("{web_base}/cards/{}", self.id.as_str().unwrap_or(&self.id.to_string())),
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
    pub(super) async fn new(domain: &str) -> Result<Self> {
        let base_url = format!("https://projects.{domain}/api");
        let hydra_token = get_token().await?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| SunbeamError::network(format!("Failed to build HTTP client: {e}")))?;

        // Exchange the Hydra access token for a Planka JWT via our custom endpoint.
        let exchange_url = format!("{base_url}/access-tokens/exchange-using-token");
        let exchange_resp = http
            .post(&exchange_url)
            .json(&serde_json::json!({ "token": hydra_token }))
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka token exchange failed: {e}")))?;

        if !exchange_resp.status().is_success() {
            let status = exchange_resp.status();
            let body = exchange_resp.text().await.unwrap_or_default();
            return Err(SunbeamError::identity(format!(
                "Planka token exchange returned {status}: {body}"
            )));
        }

        let body: serde_json::Value = exchange_resp.json().await?;
        let token = body
            .get("item")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SunbeamError::identity("Planka exchange response missing 'item' field"))?
            .to_string();

        Ok(Self {
            base_url,
            token,
            http,
        })
    }

    /// Discover all projects and boards, then fetch cards from each.
    pub(super) async fn list_all_cards(&self) -> Result<Vec<Ticket>> {
        // GET /api/projects returns all projects the user has access to,
        // with included boards.
        let url = format!("{}/projects", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| SunbeamError::network(format!("Planka list projects: {e}")))?;

        if !resp.status().is_success() {
            return Err(SunbeamError::network(format!(
                "Planka GET projects returned {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp.json().await?;
        // Extract board IDs -- Planka uses string IDs (snowflake-style)
        let board_ids: Vec<String> = body
            .get("included")
            .and_then(|inc| inc.get("boards"))
            .and_then(|b| b.as_array())
            .map(|boards| {
                boards
                    .iter()
                    .filter_map(|b| {
                        b.get("id").and_then(|id| {
                            id.as_str()
                                .map(|s| s.to_string())
                                .or_else(|| id.as_u64().map(|n| n.to_string()))
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        if board_ids.is_empty() {
            return Ok(vec![]);
        }

        // Fetch cards from each board
        let mut all_tickets = Vec::new();
        for board_id in &board_ids {
            match self.list_cards(board_id).await {
                Ok(tickets) => all_tickets.extend(tickets),
                Err(e) => {
                    crate::output::warn(&format!("Planka board {board_id}: {e}"));
                }
            }
        }

        Ok(all_tickets)
    }

    /// GET /api/boards/{id} and extract all cards.
    async fn list_cards(&self, board_id: &str) -> Result<Vec<Ticket>> {
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
    pub(super) async fn get_card(&self, id: &str) -> Result<Ticket> {
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
    pub(super) async fn create_card(
        &self,
        _board_id: &str,
        list_id: &str,
        name: &str,
        description: &str,
    ) -> Result<Ticket> {
        let url = format!("{}/lists/{list_id}/cards", self.base_url);
        let body = serde_json::json!({
            "name": name,
            "description": description,
            "position": 65535,
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
    pub(super) async fn update_card(&self, id: &str, updates: &CardUpdate) -> Result<()> {
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
    #[allow(dead_code)]
    pub(super) async fn move_card(&self, id: &str, list_id: &str) -> Result<()> {
        self.update_card(
            id,
            &CardUpdate {
                list_id: Some(serde_json::json!(list_id)),
                ..Default::default()
            },
        )
        .await
    }

    /// POST /api/cards/{id}/comment-actions
    pub(super) async fn comment_card(&self, id: &str, text: &str) -> Result<()> {
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

    /// Search for a Planka user by name/username, return their ID.
    async fn resolve_user_id(&self, query: &str) -> Result<String> {
        // "me" or "self" assigns to the current user
        if query == "me" || query == "self" {
            // Get current user via the token (decode JWT or call /api/users/me equivalent)
            // Planka doesn't have /api/users/me, but we can get user from any board membership
            let projects_url = format!("{}/projects", self.base_url);
            if let Ok(resp) = self.http.get(&projects_url).bearer_auth(&self.token).send().await {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if let Some(memberships) = body.get("included")
                        .and_then(|i| i.get("boardMemberships"))
                        .and_then(|b| b.as_array())
                    {
                        if let Some(user_id) = memberships.first()
                            .and_then(|m| m.get("userId"))
                            .and_then(|v| v.as_str())
                        {
                            return Ok(user_id.to_string());
                        }
                    }
                }
            }
        }

        // Search other users (note: Planka excludes current user from search results)
        let url = format!("{}/users/search", self.base_url);
        let resp = self.http.get(&url)
            .bearer_auth(&self.token)
            .query(&[("query", query)])
            .send().await
            .map_err(|e| SunbeamError::network(format!("Planka user search: {e}")))?;
        let body: serde_json::Value = resp.json().await?;
        let users = body.get("items").and_then(|i| i.as_array());
        if let Some(users) = users {
            if let Some(user) = users.first() {
                if let Some(id) = user.get("id").and_then(|v| v.as_str()) {
                    return Ok(id.to_string());
                }
            }
        }
        Err(SunbeamError::identity(format!(
            "Planka user not found: {query} (use 'me' to assign to yourself)"
        )))
    }

    /// POST /api/cards/{id}/memberships
    pub(super) async fn assign_card(&self, id: &str, user: &str) -> Result<()> {
        // Resolve username to user ID
        let user_id = self.resolve_user_id(user).await?;
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
