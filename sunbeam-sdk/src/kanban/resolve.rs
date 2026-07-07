//! Name-to-ID resolution helpers for the Kanban CLI UX layer.
//!
//! Everywhere the CLI accepts a raw ULID identifier, users can instead supply a
//! human-readable name. If the argument already looks like an identifier it is
//! returned unchanged; otherwise the helper lists the visible entities and
//! matches by name (case-insensitive exact match).

use crate::error::{Result, ResultExt, SunbeamError};

/// Returns true if `raw` already looks like a backend identifier.
///
/// IDs are recognised in two shapes:
/// - ULIDs (Crockford base32, 26 chars)
/// - Prefixed test/production IDs such as `proj_1`, `board_abc123`
pub fn looks_like_id(raw: &str) -> bool {
    is_ulid(raw) || is_prefixed_id(raw)
}

fn is_ulid(s: &str) -> bool {
    ulid::Ulid::from_string(s).is_ok()
}

fn is_prefixed_id(s: &str) -> bool {
    let Some((prefix, rest)) = s.split_once('_') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_lowercase())
        && !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Case-insensitive exact name match.
pub fn name_matches(candidate: &str, target: &str) -> bool {
    candidate.eq_ignore_ascii_case(target)
}

/// Pick the unique matching ID or return a helpful error.
pub fn unique_match<S: std::fmt::Display>(
    matches: Vec<(String, S)>,
    kind: &str,
    raw: &str,
) -> Result<String> {
    match matches.len() {
        0 => Err(SunbeamError::Other(format!("no {kind} named {raw:?}"))),
        1 => match matches.into_iter().next() {
            Some((id, _)) => Ok(id),
            None => unreachable!(),
        },
        _ => {
            let names: Vec<_> = matches.iter().map(|(_, name)| name.to_string()).collect();
            Err(SunbeamError::Other(format!(
                "multiple {kind}s match {raw:?}: {}",
                names.join(", ")
            )))
        }
    }
}

// ----------------------------------------------------------------------------
// Project
// ----------------------------------------------------------------------------

use crate::kanban::client::ListProjectsRequest;
use crate::kanban::projects::ProjectService;

/// Resolve a project identifier from a raw string.
///
/// If `raw` is already ID-shaped it is returned unchanged. Otherwise all
/// visible projects are listed and the unique name match is returned.
pub async fn resolve_project_id(client: &mut dyn ProjectService, raw: &str) -> Result<String> {
    if looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .list_projects(tonic::Request::new(ListProjectsRequest {}))
        .await
        .with_ctx(|| "list projects to resolve name".to_string())?;
    let matches: Vec<_> = resp
        .projects
        .into_iter()
        .filter(|p| name_matches(&p.name, raw))
        .map(|p| (p.id, p.name))
        .collect();
    unique_match(matches, "project", raw)
}

// ----------------------------------------------------------------------------
// Board
// ----------------------------------------------------------------------------

use crate::kanban::boards::BoardService;
use crate::kanban::client;

/// Resolve a board identifier from a raw string within a project.
///
/// `project_id` must already be a resolved project identifier.
pub async fn resolve_board_id(
    client: &mut dyn BoardService,
    project_id: &str,
    raw: &str,
) -> Result<String> {
    if looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .list_boards(tonic::Request::new(client::ListBoardsRequest {
            project_id: project_id.to_string(),
        }))
        .await
        .with_ctx(|| format!("list boards in project {project_id} to resolve name"))?;
    let matches: Vec<_> = resp
        .boards
        .into_iter()
        .filter(|b| name_matches(&b.name, raw))
        .map(|b| (b.id, b.name))
        .collect();
    unique_match(matches, "board", raw)
}

// ----------------------------------------------------------------------------
// Card
// ----------------------------------------------------------------------------

use crate::kanban::cards::CardService;

/// Resolve a card identifier from a raw string within a board.
///
/// Matches against card title or ref (e.g. `PROJ-42`). `board_id` must already
/// be a resolved board identifier.
pub async fn resolve_card_id(
    client: &mut dyn CardService,
    board_id: &str,
    raw: &str,
) -> Result<String> {
    if looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .list_cards_by_board(tonic::Request::new(client::ListCardsByBoardRequest {
            board_id: board_id.to_string(),
            column_id: String::new(),
            cursor: String::new(),
            limit: 0,
        }))
        .await
        .with_ctx(|| format!("list cards on board {board_id} to resolve name"))?;
    let matches: Vec<_> = resp
        .cards
        .into_iter()
        .filter(|c| name_matches(&c.title, raw) || name_matches(&c.r#ref, raw))
        .map(|c| (c.id, format!("{} {}", c.r#ref, c.title)))
        .collect();
    unique_match(matches, "card", raw)
}

// ----------------------------------------------------------------------------
// Aggregated board
// ----------------------------------------------------------------------------

use crate::kanban::aggregated::AggregatedBoardService;

/// Resolve an aggregated-board identifier from a raw string.
pub async fn resolve_aggregate_id(
    client: &mut dyn AggregatedBoardService,
    raw: &str,
) -> Result<String> {
    if looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .list_aggregated_boards(tonic::Request::new(client::ListAggregatedBoardsRequest {}))
        .await
        .with_ctx(|| "list aggregated boards to resolve name".to_string())?;
    let matches: Vec<_> = resp
        .aggregated_boards
        .into_iter()
        .filter(|b| name_matches(&b.name, raw))
        .map(|b| (b.id, b.name))
        .collect();
    unique_match(matches, "aggregated board", raw)
}

// ----------------------------------------------------------------------------
// Board template
// ----------------------------------------------------------------------------

use crate::kanban::templates::TemplateService;

/// Resolve a board-template identifier from a raw string.
///
/// `project_id` is `Some` to include project-scoped templates in the search,
/// or `None` to search only global templates.
pub async fn resolve_template_id(
    client: &mut dyn TemplateService,
    project_id: Option<&str>,
    raw: &str,
) -> Result<String> {
    if looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .list_templates(client::ListTemplatesRequest {
            project_id: project_id.unwrap_or("").to_string(),
        })
        .await
        .with_ctx(|| "list templates to resolve name".to_string())?;
    let matches: Vec<_> = resp
        .templates
        .into_iter()
        .filter(|t| name_matches(&t.name, raw))
        .map(|t| (t.id, t.name))
        .collect();
    unique_match(matches, "template", raw)
}

// ----------------------------------------------------------------------------
// Card template
// ----------------------------------------------------------------------------

use crate::kanban::card_templates::CardTemplateService;

/// Resolve a card-template identifier from a raw string.
///
/// `project_id` is `Some` to include project-scoped templates in the search,
/// or `None` to search only global templates.
pub async fn resolve_card_template_id(
    client: &mut dyn CardTemplateService,
    project_id: Option<&str>,
    raw: &str,
) -> Result<String> {
    if looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .list_card_templates(client::ListCardTemplatesRequest {
            project_id: project_id.unwrap_or("").to_string(),
        })
        .await
        .with_ctx(|| "list card templates to resolve name".to_string())?;
    let matches: Vec<_> = resp
        .templates
        .into_iter()
        .filter(|t| name_matches(&t.name, raw))
        .map(|t| (t.id, t.name))
        .collect();
    unique_match(matches, "card template", raw)
}

// ----------------------------------------------------------------------------
// Public board
// ----------------------------------------------------------------------------

use crate::kanban::public_boards::PublicBoardService;

/// Resolve a public-board identifier from a raw string within a project.
pub async fn resolve_public_board_id(
    client: &mut dyn PublicBoardService,
    project_id: &str,
    raw: &str,
) -> Result<String> {
    if looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .list_public_boards(client::ListPublicBoardsRequest {
            project_id: project_id.to_string(),
        })
        .await
        .with_ctx(|| format!("list public boards in project {project_id} to resolve name"))?;
    let matches: Vec<_> = resp
        .boards
        .into_iter()
        .filter(|b| name_matches(&b.name, raw))
        .map(|b| (b.id, b.name))
        .collect();
    unique_match(matches, "public board", raw)
}

// ----------------------------------------------------------------------------
// Search
// ----------------------------------------------------------------------------

use crate::kanban::search::SearchService;

// ----------------------------------------------------------------------------
// Dispatch-level resolver
// ----------------------------------------------------------------------------

use crate::logger::Logger;

/// Builds the required Kanban service clients and resolves parent-context names
/// before the per-service `run()` functions are invoked.
pub struct NameResolver<'a> {
    logger: &'a Logger,
    server: &'a str,
    token: &'a str,
}

impl<'a> NameResolver<'a> {
    /// Create a resolver for the given server and token.
    pub fn new(logger: &'a Logger, server: &'a str, token: &'a str) -> Self {
        Self {
            logger,
            server,
            token,
        }
    }

    /// Resolve a project name (or return the ID unchanged).
    pub async fn project(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut client =
            crate::kanban::projects::build_client(self.logger, self.server, self.token).await?;
        resolve_project_id(&mut client, raw).await
    }

    /// Resolve a board name within a project (or return the ID unchanged).
    pub async fn board(&self, project_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut client =
            crate::kanban::boards::build_client(self.logger, self.server, self.token).await?;
        resolve_board_id(&mut client, project_id, raw).await
    }

    /// Resolve a card name within a board (or return the ID unchanged).
    pub async fn card(&self, board_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut client =
            crate::kanban::cards::build_client(self.logger, self.server, self.token).await?;
        resolve_card_id(&mut client, board_id, raw).await
    }

    /// Resolve an aggregated-board name (or return the ID unchanged).
    pub async fn aggregate(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut client =
            crate::kanban::aggregated::build_client(self.logger, self.server, self.token).await?;
        resolve_aggregate_id(&mut client, raw).await
    }

    /// Resolve a board-template name (or return the ID unchanged).
    pub async fn template(&self, project_id: Option<&str>, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut client =
            crate::kanban::templates::build_client(self.logger, self.server, self.token).await?;
        resolve_template_id(&mut client, project_id, raw).await
    }

    /// Resolve a card-template name (or return the ID unchanged).
    pub async fn card_template(&self, project_id: Option<&str>, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut client =
            crate::kanban::card_templates::build_client(self.logger, self.server, self.token)
                .await?;
        resolve_card_template_id(&mut client, project_id, raw).await
    }

    /// Resolve a public-board name within a project (or return the ID unchanged).
    pub async fn public_board(&self, project_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut client =
            crate::kanban::public_boards::build_client(self.logger, self.server).await?;
        resolve_public_board_id(&mut client, project_id, raw).await
    }

    /// Resolve a board name anywhere the caller can see.
    ///
    /// This lists every visible project and every visible board within those
    /// projects until a unique name match is found.
    pub async fn board_anywhere(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut projects_client =
            crate::kanban::projects::build_client(self.logger, self.server, self.token).await?;
        let projects = projects_client
            .list_projects(tonic::Request::new(client::ListProjectsRequest {}))
            .await
            .with_ctx(|| "list projects to discover board name".to_string())?
            .projects;

        let mut matches = Vec::new();
        for project in projects {
            let mut boards_client =
                crate::kanban::boards::build_client(self.logger, self.server, self.token).await?;
            let boards = boards_client
                .list_boards(tonic::Request::new(client::ListBoardsRequest {
                    project_id: project.id.clone(),
                }))
                .await;
            let Ok(resp) = boards else {
                continue;
            };
            for board in resp.boards {
                if name_matches(&board.name, raw) {
                    matches.push((
                        board.id,
                        format!("{} (project: {})", board.name, project.name),
                    ));
                }
            }
        }
        unique_match(matches, "board", raw)
    }

    /// Resolve a card name anywhere the caller can see.
    ///
    /// Uses full-text search and then filters for an exact title or ref match.
    pub async fn card_anywhere(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut search_client =
            crate::kanban::search::build_client(self.logger, self.server, self.token).await?;
        let resp = search_client
            .search_cards(client::SearchCardsRequest {
                query: raw.to_string(),
                limit: 50,
                ..Default::default()
            })
            .await
            .with_ctx(|| "search cards to resolve name".to_string())?;
        let matches: Vec<_> = resp
            .hits
            .into_iter()
            .filter(|h| name_matches(&h.title, raw) || name_matches(&h.card_ref, raw))
            .map(|h| {
                (
                    h.card_id,
                    format!("{} {} (board: {})", h.card_ref, h.title, h.board_id),
                )
            })
            .collect();
        unique_match(matches, "card", raw)
    }

    /// Resolve a public-board name anywhere it is visible.
    ///
    /// This lists every visible project and every public board within those
    /// projects until a unique name match is found.
    pub async fn public_board_anywhere(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let mut projects_client =
            crate::kanban::projects::build_client(self.logger, self.server, self.token).await?;
        let projects = projects_client
            .list_projects(tonic::Request::new(client::ListProjectsRequest {}))
            .await
            .with_ctx(|| "list projects to discover public board name".to_string())?
            .projects;

        let mut matches = Vec::new();
        for project in projects {
            let mut pb_client =
                crate::kanban::public_boards::build_client(self.logger, self.server).await?;
            let boards = pb_client
                .list_public_boards(client::ListPublicBoardsRequest {
                    project_id: project.id.clone(),
                })
                .await;
            let Ok(resp) = boards else {
                continue;
            };
            for board in resp.boards {
                if name_matches(&board.name, raw) {
                    matches.push((
                        board.id,
                        format!("{} (project: {})", board.name, project.name),
                    ));
                }
            }
        }
        unique_match(matches, "public board", raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_id_recognises_ulids() {
        assert!(looks_like_id("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
    }

    #[test]
    fn looks_like_id_recognises_prefixed_ids() {
        assert!(looks_like_id("proj_1"));
        assert!(looks_like_id("board_abc123"));
        assert!(looks_like_id("card_1"));
        assert!(looks_like_id("agg_1"));
        assert!(looks_like_id("tmpl_1"));
        assert!(looks_like_id("ctmpl_1"));
    }

    #[test]
    fn looks_like_id_rejects_names_and_uuids() {
        assert!(!looks_like_id("Sunbeam"));
        assert!(!looks_like_id("Backlog"));
        assert!(!looks_like_id("Fix frontend crash"));
        assert!(!looks_like_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!looks_like_id("550E8400-E29B-41D4-A716-446655440000"));
    }

    #[test]
    fn name_matches_is_case_insensitive() {
        assert!(name_matches("Sunbeam", "sunbeam"));
        assert!(name_matches("Backlog", "BACKLOG"));
        assert!(!name_matches("Sunbeam", "Moonlight"));
    }

    #[test]
    fn unique_match_zero_errors() {
        let err = unique_match::<String>(Vec::new(), "project", "Sunbeam").unwrap_err();
        assert!(err.to_string().contains("no project named"));
    }

    #[test]
    fn unique_match_one_succeeds() {
        let id = unique_match(
            vec![("id".to_string(), "Sunbeam".to_string())],
            "project",
            "Sunbeam",
        )
        .unwrap();
        assert_eq!(id, "id");
    }

    #[test]
    fn unique_match_many_errors() {
        let err = unique_match(
            vec![
                ("a".to_string(), "Sunbeam".to_string()),
                ("b".to_string(), "Sunbeam 2".to_string()),
            ],
            "project",
            "Sun",
        )
        .unwrap_err();
        assert!(err.to_string().contains("multiple projects match"));
    }
}
