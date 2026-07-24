//! Name-to-ID resolution helpers for the Kanban CLI UX layer.
//!
//! Everywhere the CLI accepts a raw ULID identifier, users can instead supply
//! a human-readable reference. An argument is resolved against the visible
//! entities by, in no particular order:
//!
//! - exact ULID or legacy UUID (returned unchanged, no RPC)
//! - ULID prefix (e.g. `01KY0QK98`)
//! - exact name (case-insensitive)
//! - project key/prefix (e.g. `TRI`) for projects
//!
//! Ambiguous input fails listing the candidates (name + id); unknown input
//! fails listing the available names.

use sdk::error::{Result, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::v1;

/// Returns true if `raw` already looks like a backend identifier.
///
/// IDs are recognised in three shapes:
/// - ULIDs (Crockford base32, 26 chars)
/// - Legacy UUIDs (8-4-4-4-12 hex; some entities, e.g. board templates,
///   still carry these — `template list` prints them, so they must resolve)
/// - Prefixed test/production IDs such as `proj_1`, `board_abc123`
pub(crate) fn looks_like_id(raw: &str) -> bool {
    is_ulid(raw) || is_uuid(raw) || is_prefixed_id(raw)
}

fn is_ulid(s: &str) -> bool {
    ulid::Ulid::from_string(s).is_ok()
}

fn is_uuid(s: &str) -> bool {
    let mut segments = s.split('-');
    let lengths = [8, 4, 4, 4, 12];
    for want in lengths {
        match segments.next() {
            Some(seg) if seg.len() == want && seg.chars().all(|c| c.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    segments.next().is_none()
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

/// Returns true if `raw` is a Crockford base32 prefix of `id`.
///
/// Anything decoding as a full ULID never reaches here ([`looks_like_id`]
/// passes it through unchanged), so this covers proper prefixes only. A
/// minimum length of 4 keeps single-character input from prefix-matching
/// half the alphabet; shorter input can still match by name.
fn id_prefix_matches(id: &str, raw: &str) -> bool {
    raw.len() >= 4
        && raw.len() < 26
        && raw.chars().all(|c| {
            matches!(c.to_ascii_uppercase(), '0'..='9' | 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z')
        })
        && id.len() > raw.len()
        && id.to_ascii_uppercase().starts_with(&raw.to_ascii_uppercase())
}

/// Case-insensitive exact name match.
pub(crate) fn name_matches(candidate: &str, target: &str) -> bool {
    candidate.eq_ignore_ascii_case(target)
}

/// Match an entity by name or ULID prefix.
fn entity_matches(id: &str, name: &str, raw: &str) -> bool {
    name_matches(name, raw) || id_prefix_matches(id, raw)
}

/// How many candidates to list in error messages before truncating.
const MAX_LISTED: usize = 10;

/// Format a candidate list for an error message, truncated past [`MAX_LISTED`].
fn format_candidates<S: std::fmt::Display>(labels: &[S]) -> String {
    let shown: Vec<_> = labels
        .iter()
        .take(MAX_LISTED)
        .map(|l| l.to_string())
        .collect();
    let mut out = shown.join(", ");
    if labels.len() > MAX_LISTED {
        out.push_str(&format!(" … and {} more", labels.len() - MAX_LISTED));
    }
    out
}

/// Pick the unique matching ID or return a helpful error.
///
/// `matches` holds the `(id, label)` pairs that matched the user input;
/// `available` labels every candidate the user could have meant, and is
/// listed (names only) when nothing matched.
pub(crate) fn unique_match<S: std::fmt::Display>(
    matches: Vec<(String, S)>,
    kind: &str,
    raw: &str,
    available: &[S],
) -> Result<String> {
    match matches.len() {
        0 => {
            let mut msg = format!("no {kind} matches {raw:?}");
            if !available.is_empty() {
                msg.push_str(&format!(
                    "; available {kind}s: {}",
                    format_candidates(available)
                ));
            }
            Err(SunbeamError::Other(msg))
        }
        1 => match matches.into_iter().next() {
            Some((id, _)) => Ok(id),
            None => unreachable!(),
        },
        _ => {
            let candidates: Vec<_> = matches
                .iter()
                .map(|(id, name)| format!("{name} ({id})"))
                .collect();
            Err(SunbeamError::Other(format!(
                "multiple {kind}s match {raw:?}: {}",
                candidates.join(", ")
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

    /// Resolve a project reference (ID, ULID prefix, name, or project key).
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
        let available: Vec<String> = resp.projects.iter().map(|p| p.name.clone()).collect();
        let matches: Vec<_> = resp
            .projects
            .into_iter()
            .filter(|p| {
                entity_matches(&p.id, &p.name, raw)
                    || (!p.prefix.is_empty() && name_matches(&p.prefix, raw))
            })
            .map(|p| (p.id, p.name))
            .collect();
        unique_match(matches, "project", raw, &available)
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
        let available: Vec<String> = resp.boards.iter().map(|b| b.name.clone()).collect();
        let matches: Vec<_> = resp
            .boards
            .into_iter()
            .filter(|b| entity_matches(&b.id, &b.name, raw))
            .map(|b| (b.id, b.name))
            .collect();
        unique_match(matches, "board", raw, &available)
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
        let available: Vec<String> = resp
            .cards
            .iter()
            .map(|c| format!("{} {}", c.r#ref, c.title))
            .collect();
        let matches: Vec<_> = resp
            .cards
            .into_iter()
            .filter(|c| {
                name_matches(&c.title, raw)
                    || name_matches(&c.r#ref, raw)
                    || id_prefix_matches(&c.id, raw)
            })
            .map(|c| (c.id, format!("{} {}", c.r#ref, c.title)))
            .collect();
        unique_match(matches, "card", raw, &available)
    }

    /// Resolve an aggregated-board name or ULID prefix.
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
        let available: Vec<String> = resp
            .aggregated_boards
            .iter()
            .map(|b| b.name.clone())
            .collect();
        let matches: Vec<_> = resp
            .aggregated_boards
            .into_iter()
            .filter(|b| entity_matches(&b.id, &b.name, raw))
            .map(|b| (b.id, b.name))
            .collect();
        unique_match(matches, "aggregated board", raw, &available)
    }

    /// Resolve a board-template name or ULID prefix.
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
        let available: Vec<String> = resp.templates.iter().map(|t| t.name.clone()).collect();
        let matches: Vec<_> = resp
            .templates
            .into_iter()
            .filter(|t| entity_matches(&t.id, &t.name, raw))
            .map(|t| (t.id, t.name))
            .collect();
        unique_match(matches, "template", raw, &available)
    }

    /// Resolve a card-template name or ULID prefix.
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
        let available: Vec<String> = resp.templates.iter().map(|t| t.name.clone()).collect();
        let matches: Vec<_> = resp
            .templates
            .into_iter()
            .filter(|t| entity_matches(&t.id, &t.name, raw))
            .map(|t| (t.id, t.name))
            .collect();
        unique_match(matches, "card template", raw, &available)
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
        let mut available = Vec::new();
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
                let label = format!("{} (project: {})", board.name, project.name);
                if entity_matches(&board.id, &board.name, raw) {
                    matches.push((board.id, label.clone()));
                }
                available.push(label);
            }
        }
        unique_match(matches, "board", raw, &available)
    }

    /// Resolve a card title, ref, or ULID prefix anywhere the caller can see.
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
        let available: Vec<String> = resp
            .hits
            .iter()
            .map(|h| format!("{} {}", h.card_ref, h.title))
            .collect();
        let matches: Vec<_> = resp
            .hits
            .into_iter()
            .filter(|h| {
                name_matches(&h.title, raw)
                    || name_matches(&h.card_ref, raw)
                    || id_prefix_matches(&h.card_id, raw)
            })
            .map(|h| {
                (
                    h.card_id,
                    format!("{} {} (board: {})", h.card_ref, h.title, h.board_id),
                )
            })
            .collect();
        unique_match(matches, "card", raw, &available)
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
        let mut available = Vec::new();
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
                let label = format!("{} (project: {})", board.name, project.name);
                if entity_matches(&board.id, &board.name, raw) {
                    matches.push((board.id, label.clone()));
                }
                available.push(label);
            }
        }
        unique_match(matches, "public board", raw, &available)
    }

    /// Fetch the columns of a board via the board detail.
    async fn board_columns(&self, board_id: &str) -> Result<Vec<v1::Column>> {
        let resp = self
            .client
            .boards()
            .get_board(v1::GetBoardRequest {
                board_id: board_id.to_string(),
                ..Default::default()
            })
            .await?
            .into_owned();
        Ok(resp.detail.into_option().unwrap_or_default().columns)
    }

    /// Resolve a label name within a project's catalog (or return the ID
    /// unchanged).
    ///
    /// `project_id` must already be a resolved project identifier; the
    /// catalog is the global labels plus that project's labels.
    pub(crate) async fn label(&self, project_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let labels = self.project_labels(project_id).await?;
        let available: Vec<String> = labels.iter().map(|l| l.name.clone()).collect();
        let matches: Vec<_> = labels
            .into_iter()
            .filter(|l| entity_matches(&l.id, &l.name, raw))
            .map(|l| (l.id, l.name))
            .collect();
        unique_match(matches, "label", raw, &available)
    }

    /// Fetch the label catalog visible to a project (global + project).
    pub(crate) async fn project_labels(&self, project_id: &str) -> Result<Vec<v1::Label>> {
        let resp = self
            .client
            .labels()
            .list_labels(v1::ListLabelsRequest {
                project_id: project_id.to_string(),
                ..Default::default()
            })
            .await?
            .into_owned();
        Ok(resp.labels)
    }

    /// Resolve a milestone title within a project (or return the ID
    /// unchanged).
    ///
    /// `project_id` must already be a resolved project identifier.
    pub(crate) async fn milestone(&self, project_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let resp = self
            .client
            .milestones()
            .list_milestones(v1::ListMilestonesRequest {
                project_id: project_id.to_string(),
                ..Default::default()
            })
            .await?
            .into_owned();
        let available: Vec<String> = resp.milestones.iter().map(|m| m.title.clone()).collect();
        let matches: Vec<_> = resp
            .milestones
            .into_iter()
            .filter(|m| entity_matches(&m.id, &m.title, raw))
            .map(|m| (m.id, m.title))
            .collect();
        unique_match(matches, "milestone", raw, &available)
    }

    /// Resolve a column title or ULID prefix within a board.
    ///
    /// Failures list the board's columns (title + id) so the user can pick a
    /// valid value without a second lookup.
    pub(crate) async fn column(&self, board_id: &str, raw: &str) -> Result<String> {
        if looks_like_id(raw) {
            return Ok(raw.to_string());
        }
        let columns = self.board_columns(board_id).await?;
        let available: Vec<String> = columns
            .iter()
            .map(|c| format!("{} {}", c.title, c.id))
            .collect();
        let matches: Vec<_> = columns
            .into_iter()
            .filter(|c| entity_matches(&c.id, &c.title, raw))
            .map(|c| (c.id, c.title))
            .collect();
        unique_match(matches, "column", raw, &available)
    }

    /// Resolve the target column for `card create`.
    ///
    /// Without an explicit `--column`, the board's left-most (lowest-position)
    /// column is used — matching how triage cards are filed. Only a board
    /// with no columns at all fails.
    pub(crate) async fn column_for_create(
        &self,
        board_id: &str,
        raw: Option<&str>,
    ) -> Result<String> {
        if let Some(raw) = raw {
            return self.column(board_id, raw).await;
        }
        let columns = self.board_columns(board_id).await?;
        columns
            .iter()
            .min_by_key(|c| c.position)
            .map(|c| c.id.clone())
            .ok_or_else(|| {
                SunbeamError::Other(
                    "required: --column <ID|name> (the board has no columns)".to_string(),
                )
            })
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
    fn looks_like_id_recognises_uuids() {
        assert!(looks_like_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(looks_like_id("550E8400-E29B-41D4-A716-446655440000"));
    }

    #[test]
    fn looks_like_id_rejects_names() {
        assert!(!looks_like_id("Sunbeam"));
        assert!(!looks_like_id("Backlog"));
        assert!(!looks_like_id("Fix frontend crash"));
        assert!(!looks_like_id("550e8400-e29b-41d4-a716"));
        assert!(!looks_like_id("550e8400e29b41d4a716446655440000"));
    }

    #[test]
    fn id_prefix_matches_ulid_prefixes() {
        let id = "01KY0QK9800000000000000000";
        assert!(id_prefix_matches(id, "01KY0QK98"));
        assert!(id_prefix_matches(id, "01ky0qk98"));
        assert!(!id_prefix_matches(id, "01K"));
        assert!(!id_prefix_matches(id, id));
        assert!(!id_prefix_matches(id, "backlog"));
        assert!(!id_prefix_matches("board_1", "board"));
    }

    #[test]
    fn name_matches_is_case_insensitive() {
        assert!(name_matches("Sunbeam", "sunbeam"));
        assert!(name_matches("Backlog", "BACKLOG"));
        assert!(!name_matches("Sunbeam", "Moonlight"));
    }

    #[test]
    fn unique_match_zero_errors_and_lists_available() {
        let err = unique_match::<String>(
            Vec::new(),
            "project",
            "Moonlight",
            &["Sunbeam".to_string(), "Trident".to_string()],
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no project matches"), "{msg}");
        assert!(msg.contains("Sunbeam"), "{msg}");
        assert!(msg.contains("Trident"), "{msg}");
    }

    #[test]
    fn unique_match_one_succeeds() {
        let id = unique_match(
            vec![("id".to_string(), "Sunbeam".to_string())],
            "project",
            "Sunbeam",
            &[],
        )
        .unwrap();
        assert_eq!(id, "id");
    }

    #[test]
    fn unique_match_many_errors_with_name_and_id() {
        let err = unique_match(
            vec![
                ("id_a".to_string(), "Sunbeam".to_string()),
                ("id_b".to_string(), "Sunbeam 2".to_string()),
            ],
            "project",
            "Sun",
            &[],
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple projects match"), "{msg}");
        assert!(msg.contains("Sunbeam (id_a)"), "{msg}");
        assert!(msg.contains("Sunbeam 2 (id_b)"), "{msg}");
    }

    #[test]
    fn format_candidates_truncates_long_lists() {
        let labels: Vec<String> = (0..15).map(|i| format!("n{i}")).collect();
        let out = format_candidates(&labels);
        assert!(out.contains("n9"), "{out}");
        assert!(!out.contains("n10"), "{out}");
        assert!(out.contains("and 5 more"), "{out}");
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
        assert_eq!(
            resolver
                .template(None, "550e8400-e29b-41d4-a716-446655440000")
                .await
                .unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(resolver.card_template("ctmpl_1").await.unwrap(), "ctmpl_1");
        assert_eq!(
            resolver.public_board_anywhere("board_1").await.unwrap(),
            "board_1"
        );
    }

    /// Mount a ListProjects responder returning the given projects.
    async fn mount_projects(server: &MockServer, projects: Vec<v1::Project>) {
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects,
                ..Default::default()
            }))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn project_resolves_by_name() {
        let server = MockServer::start().await;
        mount_projects(&server, vec![project("proj_1", "Sunbeam")]).await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(resolver.project("sunbeam").await.unwrap(), "proj_1");
    }

    #[tokio::test]
    async fn project_resolves_by_key_and_ulid_prefix() {
        let server = MockServer::start().await;
        let p = v1::Project {
            id: "01KY0QK9800000000000000000".into(),
            name: "Trident".into(),
            prefix: "TRI".into(),
            ..Default::default()
        };
        mount_projects(&server, vec![p]).await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(
            resolver.project("tri").await.unwrap(),
            "01KY0QK9800000000000000000"
        );
        assert_eq!(
            resolver.project("01KY0QK98").await.unwrap(),
            "01KY0QK9800000000000000000"
        );
    }

    #[tokio::test]
    async fn project_miss_and_ambiguous() {
        let server = MockServer::start().await;
        mount_projects(
            &server,
            vec![project("proj_1", "Sunbeam"), project("proj_2", "SUNBEAM")],
        )
        .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        let err = resolver.project("sunbeam").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("multiple projects match"), "{msg}");
        assert!(msg.contains("proj_1"), "{msg}");
        let err = resolver.project("moonlight").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no project matches"), "{msg}");
        assert!(msg.contains("available projects: Sunbeam"), "{msg}");
    }

    #[tokio::test]
    async fn board_anywhere_searches_all_projects() {
        let server = MockServer::start().await;
        mount_projects(&server, vec![project("proj_1", "One")]).await;
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
        mount_projects(&server, vec![project("proj_1", "One")]).await;
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

    fn column(id: &str, title: &str) -> v1::Column {
        v1::Column {
            id: id.to_string(),
            board_id: "board_1".into(),
            title: title.to_string(),
            position: 1,
            ..Default::default()
        }
    }

    async fn mount_board_detail(server: &MockServer, columns: Vec<v1::Column>) {
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/GetBoard"))
            .respond_with(testutil::proto_response(&v1::GetBoardResponse {
                detail: v1::BoardDetail {
                    board: v1::Board {
                        id: "board_1".into(),
                        name: "Backlog".into(),
                        ..Default::default()
                    }
                    .into(),
                    columns,
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn column_resolves_by_title_and_lists_columns_on_miss() {
        let server = MockServer::start().await;
        mount_board_detail(
            &server,
            vec![column("col_1", "cli-test"), column("col_2", "backlog")],
        )
        .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(
            resolver.column("board_1", "BACKLOG").await.unwrap(),
            "col_2"
        );
        let err = resolver.column("board_1", "done").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no column matches"), "{msg}");
        assert!(msg.contains("cli-test col_1"), "{msg}");
        assert!(msg.contains("backlog col_2"), "{msg}");
    }

    #[tokio::test]
    async fn column_for_create_defaults_to_single_column() {
        let server = MockServer::start().await;
        mount_board_detail(&server, vec![column("col_only", "Todo")]).await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(
            resolver.column_for_create("board_1", None).await.unwrap(),
            "col_only"
        );
    }

    #[tokio::test]
    async fn column_for_create_defaults_to_leftmost_column() {
        let server = MockServer::start().await;
        let mut doing = column("col_2", "Doing");
        doing.position = 3;
        let mut todo = column("col_1", "Todo");
        todo.position = 0;
        // Server order is not position order; the lowest position must win.
        mount_board_detail(&server, vec![doing, todo]).await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(
            resolver.column_for_create("board_1", None).await.unwrap(),
            "col_1"
        );
    }

    #[tokio::test]
    async fn column_for_create_fails_without_columns() {
        let server = MockServer::start().await;
        mount_board_detail(&server, Vec::new()).await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        let err = resolver
            .column_for_create("board_1", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no columns"), "{err}");
    }

    #[tokio::test]
    async fn column_for_create_explicit_resolves_title() {
        let server = MockServer::start().await;
        mount_board_detail(
            &server,
            vec![column("col_1", "cli-test"), column("col_2", "backlog")],
        )
        .await;

        let client = testutil::client_for(&server.uri());
        let resolver = NameResolver::new(&client);
        assert_eq!(
            resolver
                .column_for_create("board_1", Some("cli-test"))
                .await
                .unwrap(),
            "col_1"
        );
    }
}
