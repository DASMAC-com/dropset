//! A thin, blocking Linear GraphQL client — just the two calls the binary
//! needs: read the project's open Backlog (with parents and blocking
//! relations), and rewrite the Task Staging document.
//!
//! The interactive `claude.ai` Linear MCP rides OAuth and won't authenticate
//! in a headless binary, so this path uses a personal API key from
//! `LINEAR_API_KEY`, sent verbatim as the `Authorization` header.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::merge::MergeIssue;
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

/// Full detail for one merge-group member, addressed by identifier. Carries the
/// UUID (the mutations' target) and the team (for the duplicate state lookup).
const MERGE_ISSUE_QUERY: &str = r#"
query MergeIssue($id: String!) {
  issue(id: $id) {
    id
    identifier
    description
    priority
    team { id }
    relations { nodes { type relatedIssue { identifier } } }
    inverseRelations { nodes { type issue { identifier } } }
  }
}
"#;

/// Resolve an identifier to its UUID — for an external blocking endpoint that
/// isn't itself in the merge group (so wasn't fetched in full).
const RESOLVE_UUID_QUERY: &str = r#"
query ResolveUuid($id: String!) { issue(id: $id) { id } }
"#;

/// The team's workflow states, to find the `canceled`-type state a closed
/// duplicate moves to.
const TEAM_STATES_QUERY: &str = r#"
query TeamStates($id: String!) {
  team(id: $id) { states { nodes { id name type } } }
}
"#;

const ISSUE_UPDATE_MUTATION: &str = r#"
mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) { success }
}
"#;

const RELATION_CREATE_MUTATION: &str = r#"
mutation RelationCreate($input: IssueRelationCreateInput!) {
  issueRelationCreate(input: $input) { success }
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

    /// Fetch every member of a merge group in full, plus the team id used to
    /// resolve the duplicate state. Errors if any identifier doesn't resolve,
    /// or if the members span more than one team — the duplicate close moves
    /// every member to a single team's canceled state, so a cross-team group
    /// (usually a typo'd id) would file a member into the wrong team's state.
    pub fn fetch_merge_group(&self, ids: &[String]) -> Result<(Vec<MergeIssue>, String)> {
        let mut members = Vec::with_capacity(ids.len());
        let mut team_id: Option<String> = None;
        for id in ids {
            let data: MergeIssueData = self
                .post(MERGE_ISSUE_QUERY, serde_json::json!({ "id": id }))
                .with_context(|| format!("fetching {id} for merge"))?;
            let raw = data
                .issue
                .ok_or_else(|| anyhow!("no issue resolves to {id}"))?;
            match &team_id {
                Some(first) if first != &raw.team.id => {
                    anyhow::bail!(
                        "merge group spans more than one team ({id} is not on the \
                         first member's team); refusing — check for a typo'd id"
                    );
                }
                Some(_) => {}
                None => team_id = Some(raw.team.id.clone()),
            }
            members.push(raw_to_merge_issue(raw));
        }
        let team_id = team_id.ok_or_else(|| anyhow!("merge group is empty"))?;
        Ok((members, team_id))
    }

    /// Resolve an issue identifier to its UUID.
    pub fn resolve_uuid(&self, id: &str) -> Result<String> {
        let data: ResolveUuidData = self
            .post(RESOLVE_UUID_QUERY, serde_json::json!({ "id": id }))
            .with_context(|| format!("resolving the UUID for {id}"))?;
        data.issue
            .map(|i| i.id)
            .ok_or_else(|| anyhow!("no issue resolves to {id}"))
    }

    /// The UUID of the team's duplicate / canceled state, where a closed
    /// duplicate lands. Prefers a state literally named "Duplicate", else the
    /// first `canceled`-type state.
    pub fn duplicate_state_id(&self, team_id: &str) -> Result<String> {
        let data: TeamStatesData = self
            .post(TEAM_STATES_QUERY, serde_json::json!({ "id": team_id }))
            .context("reading the team's workflow states")?;
        let states = data
            .team
            .ok_or_else(|| anyhow!("team {team_id} not found"))?
            .states
            .nodes;
        let canceled: Vec<&WorkflowState> =
            states.iter().filter(|s| s.kind == "canceled").collect();
        canceled
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case("duplicate"))
            .or_else(|| canceled.first())
            .map(|s| s.id.clone())
            .ok_or_else(|| anyhow!("team {team_id} has no canceled-type workflow state"))
    }

    /// Rewrite the canonical issue's description and priority — the
    /// write-before-close fold that must succeed before any member closes.
    pub fn update_fold(&self, uuid: &str, description: &str, priority: i64) -> Result<()> {
        self.issue_update(
            uuid,
            serde_json::json!({ "description": description, "priority": priority }),
        )
    }

    /// Move an issue to a workflow state (used to close a duplicate).
    pub fn set_state(&self, uuid: &str, state_id: &str) -> Result<()> {
        self.issue_update(uuid, serde_json::json!({ "stateId": state_id }))
    }

    /// Create a directed issue relation (`blocks` or `duplicate`): `issue_uuid`
    /// is the relation's subject, `related_uuid` its object.
    pub fn create_relation(&self, issue_uuid: &str, related_uuid: &str, kind: &str) -> Result<()> {
        let data: RelationCreateData = self.post(
            RELATION_CREATE_MUTATION,
            serde_json::json!({
                "input": {
                    "issueId": issue_uuid,
                    "relatedIssueId": related_uuid,
                    "type": kind,
                }
            }),
        )?;
        if !data.issue_relation_create.success {
            return Err(anyhow!("Linear issueRelationCreate returned success=false"));
        }
        Ok(())
    }

    /// Run an `issueUpdate` with the given input object and assert success.
    fn issue_update(&self, uuid: &str, input: serde_json::Value) -> Result<()> {
        let data: IssueUpdateData = self.post(
            ISSUE_UPDATE_MUTATION,
            serde_json::json!({ "id": uuid, "input": input }),
        )?;
        if !data.issue_update.success {
            return Err(anyhow!("Linear issueUpdate returned success=false"));
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

/// Map a raw merge-group issue into a [`MergeIssue`]. `blocks` are the forward
/// `blocks` relations; `blocked_by` are the inverse ones.
fn raw_to_merge_issue(raw: RawMergeIssue) -> MergeIssue {
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
    MergeIssue {
        number: model::parse_number(&raw.identifier).unwrap_or(0),
        uuid: raw.id,
        description: raw.description.unwrap_or_default(),
        // Linear sends priority as a float (0–4); the API takes an int back.
        priority: raw.priority.unwrap_or(0.0) as i64,
        blocked_by,
        blocks,
        id: raw.identifier,
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

#[derive(Deserialize)]
struct MergeIssueData {
    issue: Option<RawMergeIssue>,
}

#[derive(Deserialize)]
struct RawMergeIssue {
    id: String,
    identifier: String,
    description: Option<String>,
    priority: Option<f64>,
    team: IdRef,
    relations: RelationConn,
    #[serde(rename = "inverseRelations")]
    inverse_relations: InverseRelationConn,
}

#[derive(Deserialize)]
struct IdRef {
    id: String,
}

#[derive(Deserialize)]
struct ResolveUuidData {
    issue: Option<IdRef>,
}

#[derive(Deserialize)]
struct TeamStatesData {
    team: Option<Team>,
}

#[derive(Deserialize)]
struct Team {
    states: WorkflowStateConn,
}

#[derive(Deserialize)]
struct WorkflowStateConn {
    nodes: Vec<WorkflowState>,
}

#[derive(Deserialize)]
struct WorkflowState {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct IssueUpdateData {
    #[serde(rename = "issueUpdate")]
    issue_update: IssueUpdate,
}

#[derive(Deserialize)]
struct IssueUpdate {
    success: bool,
}

#[derive(Deserialize)]
struct RelationCreateData {
    #[serde(rename = "issueRelationCreate")]
    issue_relation_create: RelationCreate,
}

#[derive(Deserialize)]
struct RelationCreate {
    success: bool,
}
