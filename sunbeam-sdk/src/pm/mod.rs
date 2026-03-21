//! Unified project management across Planka (kanban boards) and Gitea (issues).
//!
//! Ticket IDs use a prefix format:
//! - `p:42` or `planka:42` -- Planka card
//! - `g:studio/cli#7` or `gitea:studio/cli#7` -- Gitea issue

mod planka;
mod gitea_issues;

use planka::PlankaClient;
use gitea_issues::GiteaClient;

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
    /// Planka card by ID (snowflake string).
    Planka(String),
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
            if rest.is_empty() {
                return Err(SunbeamError::config("Empty Planka card ID"));
            }
            Ok(TicketRef::Planka(rest.to_string()))
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
    let domain = crate::config::domain();
    if domain.is_empty() { return Err(crate::error::SunbeamError::config("No domain configured. Run: sunbeam config set --domain sunbeam.pt")); }

    let fetch_planka = source.is_none() || matches!(source, Some("planka" | "p"));
    let fetch_gitea = source.is_none() || matches!(source, Some("gitea" | "g"));

    let planka_fut = async {
        if fetch_planka {
            let client = PlankaClient::new(&domain).await?;
            client.list_all_cards().await
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
    let domain = crate::config::domain();
    if domain.is_empty() { return Err(crate::error::SunbeamError::config("No domain configured. Run: sunbeam config set --domain sunbeam.pt")); }
    let ticket_ref = parse_ticket_id(id)?;

    let ticket = match ticket_ref {
        TicketRef::Planka(card_id) => {
            let client = PlankaClient::new(&domain).await?;
            client.get_card(&card_id).await?
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
    let domain = crate::config::domain();
    if domain.is_empty() { return Err(crate::error::SunbeamError::config("No domain configured. Run: sunbeam config set --domain sunbeam.pt")); }

    let ticket = match source {
        "planka" | "p" => {
            let client = PlankaClient::new(&domain).await?;

            // Fetch all boards
            let projects_url = format!("{}/projects", client.base_url);
            let resp = client.http.get(&projects_url).bearer_auth(&client.token).send().await?;
            let projects_body: serde_json::Value = resp.json().await?;
            let boards = projects_body.get("included").and_then(|i| i.get("boards"))
                .and_then(|b| b.as_array())
                .ok_or_else(|| SunbeamError::config("No Planka boards found"))?;

            // Find the board: by name (--target "Board Name") or by ID, or use first
            let board = if target.is_empty() {
                boards.first()
            } else {
                boards.iter().find(|b| {
                    let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    name.eq_ignore_ascii_case(target) || id == target
                }).or_else(|| boards.first())
            }.ok_or_else(|| SunbeamError::config("No Planka boards found"))?;

            let board_id = board.get("id").and_then(|v| v.as_str())
                .ok_or_else(|| SunbeamError::config("Board has no ID"))?;
            let board_name = board.get("name").and_then(|n| n.as_str()).unwrap_or("?");

            // Fetch the board to get its lists, use the first list
            let board_url = format!("{}/boards/{board_id}", client.base_url);
            let board_resp = client.http.get(&board_url).bearer_auth(&client.token).send().await?;
            let board_body: serde_json::Value = board_resp.json().await?;
            let list_id = board_body.get("included").and_then(|i| i.get("lists"))
                .and_then(|l| l.as_array()).and_then(|a| a.first())
                .and_then(|l| l.get("id")).and_then(|v| v.as_str())
                .ok_or_else(|| SunbeamError::config(format!("No lists in board '{board_name}'")))?;

            client.create_card(board_id, list_id, title, body).await?
        }
        "gitea" | "g" => {
            if target.is_empty() {
                return Err(SunbeamError::config(
                    "Gitea target required: --target org/repo (e.g. studio/marathon)",
                ));
            }
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
    let domain = crate::config::domain();
    if domain.is_empty() { return Err(crate::error::SunbeamError::config("No domain configured. Run: sunbeam config set --domain sunbeam.pt")); }
    let ticket_ref = parse_ticket_id(id)?;

    match ticket_ref {
        TicketRef::Planka(card_id) => {
            let client = PlankaClient::new(&domain).await?;
            client.comment_card(&card_id, text).await?;
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
    let domain = crate::config::domain();
    if domain.is_empty() { return Err(crate::error::SunbeamError::config("No domain configured. Run: sunbeam config set --domain sunbeam.pt")); }
    let ticket_ref = parse_ticket_id(id)?;

    match ticket_ref {
        TicketRef::Planka(card_id) => {
            let client = PlankaClient::new(&domain).await?;
            // Get the card to find its board, then find a "Done"/"Closed" list
            let ticket = client.get_card(&card_id).await?;
            // Try to find the board and its lists
            let url = format!("{}/cards/{card_id}", client.base_url);
            let resp = client.http.get(&url).bearer_auth(&client.token).send().await
                .map_err(|e| SunbeamError::network(format!("Planka get card: {e}")))?;
            let body: serde_json::Value = resp.json().await?;
            let board_id = body.get("item").and_then(|i| i.get("boardId"))
                .and_then(|v| v.as_str()).unwrap_or("");

            if !board_id.is_empty() {
                // Fetch the board to get its lists
                let board_url = format!("{}/boards/{board_id}", client.base_url);
                let board_resp = client.http.get(&board_url).bearer_auth(&client.token).send().await
                    .map_err(|e| SunbeamError::network(format!("Planka get board: {e}")))?;
                let board_body: serde_json::Value = board_resp.json().await?;
                let lists = board_body.get("included")
                    .and_then(|i| i.get("lists"))
                    .and_then(|l| l.as_array());

                if let Some(lists) = lists {
                    // Find a list named "Done", "Closed", "Complete", or similar
                    let done_list = lists.iter().find(|l| {
                        let name = l.get("name").and_then(|n| n.as_str()).unwrap_or("").to_lowercase();
                        name.contains("done") || name.contains("closed") || name.contains("complete")
                    });

                    if let Some(done_list) = done_list {
                        let list_id = done_list.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        if !list_id.is_empty() {
                            client.update_card(&card_id, &planka::CardUpdate {
                                list_id: Some(serde_json::json!(list_id)),
                                ..Default::default()
                            }).await?;
                            output::ok(&format!("Moved p:{card_id} to Done."));
                            return Ok(());
                        }
                    }
                }
            }
            output::warn(&format!("Could not find a Done list for p:{card_id}. Move it manually."));
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
    let domain = crate::config::domain();
    if domain.is_empty() { return Err(crate::error::SunbeamError::config("No domain configured. Run: sunbeam config set --domain sunbeam.pt")); }
    let ticket_ref = parse_ticket_id(id)?;

    match ticket_ref {
        TicketRef::Planka(card_id) => {
            let client = PlankaClient::new(&domain).await?;
            client.assign_card(&card_id, user).await?;
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
        assert_eq!(r, TicketRef::Planka("42".to_string()));
    }

    #[test]
    fn test_parse_planka_long() {
        let r = parse_ticket_id("planka:100").unwrap();
        assert_eq!(r, TicketRef::Planka("100".to_string()));
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
        // Empty ID should fail
        assert!(parse_ticket_id("p:").is_err());
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
        assert_eq!(gitea_issues::gitea_json::state_to_status("open"), Status::Open);
    }

    #[test]
    fn test_gitea_state_closed() {
        assert_eq!(gitea_issues::gitea_json::state_to_status("closed"), Status::Closed);
    }

    #[test]
    fn test_gitea_state_unknown_defaults_open() {
        assert_eq!(gitea_issues::gitea_json::state_to_status("weird"), Status::Open);
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
                id: "p:1".to_string(),
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
                id: "g:studio/cli#7".to_string(),
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
        assert!(tbl.contains("p:1"));
        assert!(tbl.contains("g:studio/cli#7"));
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
        let update = planka::CardUpdate {
            name: Some("New name".to_string()),
            description: None,
            list_id: Some(serde_json::json!(5)),
        };
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(json["name"], "New name");
        assert_eq!(json["listId"], 5);
        assert!(json.get("description").is_none());
    }

    #[test]
    fn test_issue_update_serialization() {
        let update = gitea_issues::IssueUpdate {
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
        use planka::planka_json::*;

        let inc = BoardIncluded {
            cards: vec![],
            card_memberships: vec![],
            card_labels: vec![],
            labels: vec![],
            lists: vec![
                List { id: serde_json::json!(1), name: "To Do".to_string() },
                List { id: serde_json::json!(2), name: "In Progress".to_string() },
                List { id: serde_json::json!(3), name: "Done".to_string() },
                List { id: serde_json::json!(4), name: "Archived / Closed".to_string() },
            ],
            users: vec![],
        };

        let make_card = |list_id: u64| Card {
            id: serde_json::json!(1),
            name: "test".to_string(),
            description: None,
            list_id: Some(serde_json::json!(list_id)),
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
        let issue = gitea_issues::gitea_json::Issue {
            number: 42,
            title: "Bug report".to_string(),
            body: Some("Something broke".to_string()),
            state: "open".to_string(),
            assignees: Some(vec![gitea_issues::gitea_json::GiteaUser {
                login: "dev1".to_string(),
            }]),
            labels: Some(vec![gitea_issues::gitea_json::GiteaLabel {
                name: "bug".to_string(),
            }]),
            created_at: Some("2025-03-01T00:00:00Z".to_string()),
            updated_at: Some("2025-03-02T00:00:00Z".to_string()),
            html_url: Some("https://src.example.com/studio/app/issues/42".to_string()),
            repository: None,
        };

        let ticket = issue.to_ticket("https://src.example.com/api/v1", "studio", "app");
        assert_eq!(ticket.id, "g:studio/app#42");
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
