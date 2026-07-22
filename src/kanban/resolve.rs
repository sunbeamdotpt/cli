//! Name-to-ID resolution helpers for the Kanban CLI UX layer.
//!
//! Everywhere the CLI accepts a raw ULID identifier, users can instead supply a
//! human-readable name. If the argument already looks like an identifier it is
//! returned unchanged; otherwise the helper lists the visible entities and
//! matches by name (case-insensitive exact match).

use sdk::error::{Result, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::v1;

/// Returns true if `raw` already looks like a backend identifier.
///
/// IDs are recognised in two shapes:
/// - ULIDs (Crockford base32, 26 chars)
/// - Prefixed test/production IDs such as `proj_1`, `board_abc123`
pub(crate) fn looks_like_id(raw: &str) -> bool {
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
pub(crate) fn name_matches(candidate: &str, target: &str) -> bool {
    candidate.eq_ignore_ascii_case(target)
}

/// Pick the unique matching ID or return a helpful error.
pub(crate) fn unique_match<S: std::fmt::Display>(
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

/// Resolves human-friendly names to object IDs using the shared client.
///
/// Parent-context names (project for a board, board for a card) are resolved
/// before the per-domain `run()` functions are invoked.
pub(crate) struct NameResolver<'a> {
    client: &'a KanbanClient,
}

impl<'a> NameResolver<'a> {
    /// Create a resolver over the given client.
    pub(crate) fn new(client: &'a KanbanClient) -> Self {
        Self { client }
    }

    /// Resolve a project name (or return the ID unchanged).
    pub(crate) async fn project(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .projects()
            .list_projects(v1::ListProjectsRequest::default())
            .await?
            .into_owned();
        let matches: Vec<_> = resp
            .projects
            .into_iter()
            .filter(|p| name_matches(&p.name, raw))
            .map(|p| (p.id, p.name))
            .collect();
        unique_match(matches, "project", raw)
    }

    /// Resolve a board name within a project (or return the ID unchanged).
    ///
    /// `project_id` must already be a resolved project identifier.
    #[allow(dead_code)]
    pub(crate) async fn board(&self, project_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .boards()
            .list_boards(v1::ListBoardsRequest {
                project_id: project_id.to_string(),
                ..Default::default()
            })
            .await?
            .into_owned();
        let matches: Vec<_> = resp
            .boards
            .into_iter()
            .filter(|b| name_matches(&b.name, raw))
            .map(|b| (b.id, b.name))
            .collect();
        unique_match(matches, "board", raw)
    }

    /// Resolve a card title or ref within a board (or return the ID unchanged).
    ///
    /// `board_id` must already be a resolved board identifier.
    #[allow(dead_code)]
    pub(crate) async fn card(&self, board_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .cards()
            .list_cards_by_board(v1::ListCardsByBoardRequest {
                board_id: board_id.to_string(),
                ..Default::default()
            })
            .await?
            .into_owned();
        let matches: Vec<_> = resp
            .cards
            .into_iter()
            .filter(|c| name_matches(&c.title, raw) || name_matches(&c.r#ref, raw))
            .map(|c| (c.id, format!("{} {}", c.r#ref, c.title)))
            .collect();
        unique_match(matches, "card", raw)
    }

    /// Resolve an aggregated-board name (or return the ID unchanged).
    pub(crate) async fn aggregate(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .aggregated_boards()
            .list_aggregated_boards(v1::ListAggregatedBoardsRequest::default())
            .await?
            .into_owned();
        let matches: Vec<_> = resp
            .aggregated_boards
            .into_iter()
            .filter(|b| name_matches(&b.name, raw))
            .map(|b| (b.id, b.name))
            .collect();
        unique_match(matches, "aggregated board", raw)
    }

    /// Resolve a board-template name (or return the ID unchanged).
    ///
    /// `project_id` is `Some` to include project-scoped templates in the
    /// search, or `None` to search only global templates.
    pub(crate) async fn template(&self, project_id: Option<&str>, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .templates()
            .list_templates(v1::ListTemplatesRequest {
                project_id: project_id.unwrap_or("").to_string(),
                ..Default::default()
            })
            .await?
            .into_owned();
        let matches: Vec<_> = resp
            .templates
            .into_iter()
            .filter(|t| name_matches(&t.name, raw))
            .map(|t| (t.id, t.name))
            .collect();
        unique_match(matches, "template", raw)
    }

    /// Resolve a card-template name (or return the ID unchanged).
    ///
    /// Searches global templates (matching the old CLI behaviour of passing
    /// no project when resolving card-template names).
    pub(crate) async fn card_template(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .templates()
            .list_card_templates(v1::ListCardTemplatesRequest {
                project_id: String::new(),
                ..Default::default()
            })
            .await?
            .into_owned();
        let matches: Vec<_> = resp
            .templates
            .into_iter()
            .filter(|t| name_matches(&t.name, raw))
            .map(|t| (t.id, t.name))
            .collect();
        unique_match(matches, "card template", raw)
    }

    /// Resolve a board name anywhere the caller can see.
    ///
    /// Lists every visible project and every visible board within those
    /// projects until a unique name match is found.
    pub(crate) async fn board_anywhere(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let projects = self
            .client
            .projects()
            .list_projects(v1::ListProjectsRequest::default())
            .await?
            .into_owned()
            .projects;

        let mut matches = Vec::new();
        for project in projects {
            let boards = self
                .client
                .boards()
                .list_boards(v1::ListBoardsRequest {
                    project_id: project.id.clone(),
                    ..Default::default()
                })
                .await;
            let Ok(resp) = boards else {
                continue;
            };
            for board in resp.into_owned().boards {
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

    /// Resolve a card title or ref anywhere the caller can see.
    ///
    /// Uses full-text search and then filters for an exact title or ref match.
    pub(crate) async fn card_anywhere(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .search()
            .search_cards(v1::SearchCardsRequest {
                query: raw.to_string(),
                limit: 50,
                ..Default::default()
            })
            .await?
            .into_owned();
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
    /// Lists every visible project and every public board within those
    /// projects until a unique name match is found.
    pub(crate) async fn public_board_anywhere(&self, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let projects = self
            .client
            .projects()
            .list_projects(v1::ListProjectsRequest::default())
            .await?
            .into_owned()
            .projects;

        let mut matches = Vec::new();
        for project in projects {
            let boards = self
                .client
                .public_boards()
                .list_public_boards(v1::ListPublicBoardsRequest {
                    project_id: project.id.clone(),
                    ..Default::default()
                })
                .await;
            let Ok(resp) = boards else {
                continue;
            };
            for board in resp.into_owned().boards {
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
    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

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

    fn project(id: &str, name: &str) -> v1::Project {
        v1::Project {
            id: id.to_string(),
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn resolver_passes_ids_through_without_rpc() {
        // No mock server: ID-shaped input must not trigger any HTTP call.
        let client = testutil::client_for("http://127.0.0.1:1");
        let resolver = NameResolver::new(&client);
        assert_eq!(resolver.project("proj_1").await.unwrap(), "proj_1");
        assert_eq!(
            resolver.board_anywhere("board_abc").await.unwrap(),
            "board_abc"
        );
        assert_eq!(resolver.card_anywhere("card_1").await.unwrap(), "card_1");
        assert_eq!(resolver.aggregate("agg_1").await.unwrap(), "agg_1");
        assert_eq!(resolver.template(None, "tmpl_1").await.unwrap(), "tmpl_1");
        assert_eq!(resolver.card_template("ctmpl_1").await.unwrap(), "ctmpl_1");
        assert_eq!(
            resolver.public_board_anywhere("board_1").await.unwrap(),
            "board_1"
        );
    }

    #[tokio::test]
    async fn project_resolves_by_name() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![project("proj_1", "Sunbeam")],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(resolver.project("sunbeam").await.unwrap(), "proj_1");
    }

    #[tokio::test]
    async fn project_miss_and_ambiguous() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![project("proj_1", "Sunbeam"), project("proj_2", "SUNBEAM")],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        let err = resolver.project("sunbeam").await.unwrap_err();
        assert!(err.to_string().contains("multiple projects match"));
        let err = resolver.project("moonlight").await.unwrap_err();
        assert!(err.to_string().contains("no project named"));
    }

    #[tokio::test]
    async fn board_anywhere_searches_all_projects() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![project("proj_1", "One")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/ListBoards"))
            .respond_with(testutil::proto_response(&v1::ListBoardsResponse {
                boards: vec![v1::Board {
                    id: "board_9".into(),
                    name: "Backlog".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(resolver.board_anywhere("backlog").await.unwrap(), "board_9");
    }

    #[tokio::test]
    async fn card_anywhere_matches_title_and_ref() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.SearchService/SearchCards"))
            .respond_with(testutil::proto_response(&v1::SearchCardsResponse {
                hits: vec![v1::CardSearchHit {
                    card_id: "card_7".into(),
                    card_ref: "BEAM-7".into(),
                    title: "Fix frontend crash".into(),
                    board_id: "board_1".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(resolver.card_anywhere("BEAM-7").await.unwrap(), "card_7");
    }

    #[tokio::test]
    async fn public_board_anywhere_uses_public_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![project("proj_1", "One")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.PublicBoardService/ListPublicBoards",
            ))
            .respond_with(testutil::proto_response(&v1::ListBoardsResponse {
                boards: vec![v1::Board {
                    id: "board_pub".into(),
                    name: "Roadmap".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(
            resolver.public_board_anywhere("roadmap").await.unwrap(),
            "board_pub"
        );
    }
}
