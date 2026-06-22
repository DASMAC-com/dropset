//! A thin, blocking Linear GraphQL client — just the two calls the binary
//! needs: read the project's open Backlog (with parents and blocking
//! relations), and rewrite the Task Staging document.
//!
//! The interactive `claude.ai` Linear MCP rides OAuth and won't authenticate
//! in a headless binary, so this path uses a personal API key from
//! `LINEAR_API_KEY`, sent verbatim as the `Authorization` header.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::model::{self, Issue};

const ENDPOINT: &str = "https://api.linear.app/graphql";

/// How many Backlog issues a single query reads. The Dropset Backlog is far
/// under this; `fetch_backlog` errors rather than truncate if it's exceeded.
const PAGE_SIZE: i64 = 250;

/// Overall per-request timeout, so a hung endpoint can't wedge a `make` run.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

const BACKLOG_QUERY: &str = r#"
query Backlog($projectId: ID!, $first: Int!) {
  issues(
    filter: { project: { id: { eq: $projectId } }, state: { type: { eq: "backlog" } } }
    first: $first
  ) {
    pageInfo { hasNextPage }
    nodes {
      identifier
      description
      parent { identifier }
      relations { nodes { type relatedIssue { identifier } } }
      inverseRelations { nodes { type issue { identifier } } }
    }
  }
}
"#;

const SAVE_DOC_MUTATION: &str = r#"
mutation SaveDoc($id: String!, $content: String!) {
  documentUpdate(id: $id, input: { content: $content }) { success }
}
"#;

/// A Linear API client bound to a personal API key.
pub struct Client {
    api_key: String,
}

impl Client {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// All open Backlog issues for the project, distilled into [`Issue`]s.
    pub fn fetch_backlog(&self, project_id: &str) -> Result<Vec<Issue>> {
        let data: BacklogData = self.post(
            BACKLOG_QUERY,
            serde_json::json!({ "projectId": project_id, "first": PAGE_SIZE }),
        )?;
        // The query reads one page (PAGE_SIZE). Rather than silently stage —
        // and overwrite the document with — a truncated tree, refuse: a
        // partial plan is worse than none. (Pagination is a future
        // enhancement; the Dropset Backlog is far under this cap today.)
        if data.issues.page_info.has_next_page {
            return Err(anyhow!(
                "project has more than {PAGE_SIZE} open Backlog issues; \
                 pagination is not yet implemented, so refusing to stage a \
                 truncated tree"
            ));
        }
        Ok(data.issues.nodes.into_iter().map(raw_to_issue).collect())
    }

    /// Rewrite the Task Staging document's body in full.
    pub fn save_document(&self, id: &str, content: &str) -> Result<()> {
        let data: SaveDocData = self.post(
            SAVE_DOC_MUTATION,
            serde_json::json!({ "id": id, "content": content }),
        )?;
        if !data.document_update.success {
            return Err(anyhow!("Linear documentUpdate returned success=false"));
        }
        Ok(())
    }

    /// POST a GraphQL operation and decode `data`, surfacing transport and
    /// GraphQL-level errors with their messages.
    fn post<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        let response = match ureq::post(ENDPOINT)
            .timeout(REQUEST_TIMEOUT)
            .set("Authorization", &self.api_key)
            .set("Content-Type", "application/json")
            .send_json(body)
        {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                return Err(anyhow!("Linear API returned HTTP {code}: {detail}"));
            }
            Err(e) => return Err(anyhow!("Linear API request failed: {e}")),
        };

        let parsed: GqlResponse<T> = response
            .into_json()
            .context("decoding Linear GraphQL response")?;
        if let Some(errors) = parsed.errors {
            let joined = errors
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(anyhow!("Linear GraphQL error: {joined}"));
        }
        parsed
            .data
            .ok_or_else(|| anyhow!("Linear GraphQL response carried no data"))
    }
}

/// Map a raw GraphQL issue into the planner's [`Issue`].
fn raw_to_issue(raw: RawIssue) -> Issue {
    let blocks = raw
        .relations
        .nodes
        .into_iter()
        .filter(|r| r.kind == "blocks")
        .filter_map(|r| r.related_issue.map(|i| i.identifier))
        .collect();
    let blocked_by = raw
        .inverse_relations
        .nodes
        .into_iter()
        .filter(|r| r.kind == "blocks")
        .filter_map(|r| r.issue.map(|i| i.identifier))
        .collect();
    let touches = raw
        .description
        .as_deref()
        .map(model::parse_touches)
        .unwrap_or_default();
    Issue {
        number: model::parse_number(&raw.identifier).unwrap_or(0),
        parent: raw.parent.map(|p| p.identifier),
        id: raw.identifier,
        touches,
        blocked_by,
        blocks,
    }
}

#[derive(Deserialize)]
struct GqlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GqlError>>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Deserialize)]
struct BacklogData {
    issues: IssueConnection,
}

#[derive(Deserialize)]
struct IssueConnection {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<RawIssue>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Deserialize)]
struct RawIssue {
    identifier: String,
    description: Option<String>,
    parent: Option<IdentRef>,
    relations: RelationConn,
    #[serde(rename = "inverseRelations")]
    inverse_relations: InverseRelationConn,
}

#[derive(Deserialize)]
struct IdentRef {
    identifier: String,
}

#[derive(Deserialize)]
struct RelationConn {
    nodes: Vec<Relation>,
}

#[derive(Deserialize)]
struct Relation {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "relatedIssue")]
    related_issue: Option<IdentRef>,
}

#[derive(Deserialize)]
struct InverseRelationConn {
    nodes: Vec<InverseRelation>,
}

#[derive(Deserialize)]
struct InverseRelation {
    #[serde(rename = "type")]
    kind: String,
    issue: Option<IdentRef>,
}

#[derive(Deserialize)]
struct SaveDocData {
    #[serde(rename = "documentUpdate")]
    document_update: DocumentUpdate,
}

#[derive(Deserialize)]
struct DocumentUpdate {
    success: bool,
}
