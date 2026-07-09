//! rmcp `ServerHandler` implementation: wires the MCP tool/resource surface
//! onto the read-only [`McpDb`] data layer in `db`.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{
    Implementation, ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{
    DictationSummary, IncludeSet, McpDb, MeetingDetail, MeetingSearchHit, MeetingSummary,
};

const RESOURCE_URI_PREFIX: &str = "souffle://meeting/";

/// Number of meetings surfaced as individual `souffle://meeting/{id}`
/// resources. `list_resources` has no server-side pagination cursor of its
/// own here, so this caps the response instead of dumping the entire
/// history; `list_meetings`/`search_meetings` remain the tools for browsing
/// beyond this.
const MAX_LISTED_RESOURCES: i64 = 200;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMeetingsArgs {
    /// Optional full-text search filter (FTS5 syntax). When omitted, lists
    /// meetings newest first with no text filter.
    #[serde(default)]
    pub query: Option<String>,
    /// Inclusive lower bound on the meeting start time (ISO 8601 date or
    /// date-time).
    #[serde(default)]
    pub from: Option<String>,
    /// Inclusive upper bound on the meeting start time (ISO 8601 date or
    /// date-time).
    #[serde(default)]
    pub to: Option<String>,
    /// Maximum number of meetings to return (default 20, capped at 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMeetingArgs {
    /// Meeting id, as returned by `list_meetings` or `search_meetings`.
    pub id: String,
    /// Which sections to include: any of "transcript", "summary", "notes",
    /// "metadata". Omit (or leave empty) to include everything.
    #[serde(default)]
    pub include: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchMeetingsArgs {
    /// Full-text search query (FTS5 syntax).
    pub query: String,
    /// Maximum number of hits to return (default 20, capped at 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDictationsArgs {
    /// Maximum number of entries to return (default 20, capped at 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

// The MCP spec requires a tool's structured output schema to be a JSON
// object at the root, so list-returning tools wrap their `Vec<T>` in a
// single-field struct rather than returning the array directly.

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeetingListResult {
    pub meetings: Vec<MeetingSummary>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeetingSearchResults {
    pub results: Vec<MeetingSearchHit>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DictationListResult {
    pub entries: Vec<DictationSummary>,
}

fn clamp_limit(limit: Option<u32>, default: i64) -> i64 {
    limit.map(|l| l as i64).unwrap_or(default).clamp(1, 200)
}

#[derive(Clone)]
pub struct SouffleMcpServer {
    db: Arc<McpDb>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SouffleMcpServer {
    pub fn new(db: McpDb) -> Self {
        Self {
            db: Arc::new(db),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List meetings, newest first, with an optional full-text query and date range filter."
    )]
    async fn list_meetings(
        &self,
        Parameters(args): Parameters<ListMeetingsArgs>,
    ) -> Result<Json<MeetingListResult>, String> {
        let limit = clamp_limit(args.limit, 20);
        self.db
            .list_meetings(args.query.as_deref(), args.from.as_deref(), args.to.as_deref(), limit)
            .map(|meetings| Json(MeetingListResult { meetings }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Get a single meeting's transcript, summary, notes, and metadata by id."
    )]
    async fn get_meeting(
        &self,
        Parameters(args): Parameters<GetMeetingArgs>,
    ) -> Result<Json<MeetingDetail>, String> {
        let include = IncludeSet::from_names(args.include.as_deref());
        self.db
            .get_meeting(&args.id, include)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(description = "Full-text search across all meetings, returning matched snippets.")]
    async fn search_meetings(
        &self,
        Parameters(args): Parameters<SearchMeetingsArgs>,
    ) -> Result<Json<MeetingSearchResults>, String> {
        let limit = clamp_limit(args.limit, 20);
        self.db
            .search_meetings(&args.query, limit)
            .map(|results| Json(MeetingSearchResults { results }))
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Get the most recently recorded meeting (transcript, summary, notes, metadata)."
    )]
    async fn get_latest_meeting(&self) -> Result<Json<MeetingDetail>, String> {
        self.db
            .latest_meeting(IncludeSet::all())
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(description = "List dictation history entries, newest first.")]
    async fn list_dictations(
        &self,
        Parameters(args): Parameters<ListDictationsArgs>,
    ) -> Result<Json<DictationListResult>, String> {
        let limit = clamp_limit(args.limit, 20);
        self.db
            .list_dictations(limit)
            .map(|entries| Json(DictationListResult { entries }))
            .map_err(|e| e.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SouffleMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::from_build_env())
        .with_instructions(
            "Read-only access to Souffle meeting transcripts, summaries, notes, and dictation \
             history, stored locally in the user's Souffle app. Tools: list_meetings (browse/filter), \
             get_meeting (fetch one meeting by id), search_meetings (full-text search), \
             get_latest_meeting (most recent), list_dictations (dictation history). Also exposes \
             each meeting as a souffle://meeting/{id} resource returning its transcript."
                .to_string(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let meetings = self
            .db
            .list_meetings(None, None, None, MAX_LISTED_RESOURCES)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let resources = meetings
            .into_iter()
            .map(|m| Resource::new(format!("{RESOURCE_URI_PREFIX}{}", m.id), m.title))
            .collect();

        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(),
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();
        let Some(id) = uri.strip_prefix(RESOURCE_URI_PREFIX) else {
            return Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({ "uri": uri })),
            ));
        };

        let meeting = self.db.get_meeting(id, IncludeSet::all()).map_err(|e| {
            McpError::resource_not_found(e.to_string(), Some(json!({ "uri": uri })))
        })?;

        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            render_meeting_resource(&meeting),
            request.uri.clone(),
        )]))
    }
}

fn render_meeting_resource(meeting: &MeetingDetail) -> String {
    let mut out = format!("# {}\n\n{}\n", meeting.title, meeting.started_at);

    if let Some(summary) = meeting.summary.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str("\n## Summary\n\n");
        out.push_str(summary);
        out.push('\n');
    }

    if let Some(structured) = meeting.structured_summary.as_ref() {
        if !structured.decisions.is_empty() {
            out.push_str("\n## Decisions\n\n");
            for decision in &structured.decisions {
                out.push_str("- ");
                out.push_str(decision);
                out.push('\n');
            }
        }
        if !structured.action_items.is_empty() {
            out.push_str("\n## Action Items\n\n");
            for item in &structured.action_items {
                out.push_str("- ");
                if let Some(owner) = item.owner.as_deref().filter(|o| !o.is_empty()) {
                    out.push_str(owner);
                    out.push_str(": ");
                }
                out.push_str(&item.text);
                out.push('\n');
            }
        }
        if !structured.open_questions.is_empty() {
            out.push_str("\n## Open Questions\n\n");
            for question in &structured.open_questions {
                out.push_str("- ");
                out.push_str(question);
                out.push('\n');
            }
        }
    }

    let transcript = meeting.transcript.as_deref().unwrap_or("(no transcript)");
    out.push_str("\n## Transcript\n\n");
    out.push_str(transcript);
    out
}
