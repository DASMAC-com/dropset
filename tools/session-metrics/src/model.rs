//! Transcript parsing and token-cost aggregation — the deterministic core of
//! the `session-metrics` skill, kept pure so it unit-tests over synthetic
//! JSONL without touching the filesystem or the network.
//!
//! A Claude Code session transcript is newline-delimited JSON, one record per
//! line. The records this accounting cares about:
//!
//! * **assistant** records carry a `message.usage` block (`input_tokens`,
//!   `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`)
//!   — summed into the session totals — and a `message.content` array whose
//!   `tool_use` items name each tool call and its `id`.
//! * **user** records carry `tool_result` items in `message.content`, each
//!   keyed by `tool_use_id`; their serialized length is the result's transcript
//!   cost, attributed back to the issuing tool by that id.
//!
//! Per-result *token* counts aren't stored on disk, so result cost is
//! approximated as bytes / 4 — adequate for *ranking* sinks, which is the
//! point. Sub-agent transcripts live in sibling files and are rolled up
//! per-agent from their own `usage` blocks; their internal tool calls are not
//! attributed to the main-session tool table (the main table mirrors what the
//! main session replays in its own context).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Rough bytes-per-token divisor for approximating a result's token cost from
/// its serialized length. Labelled approximate wherever it surfaces.
const BYTES_PER_TOKEN: u64 = 4;

/// How many rows the ranked tables keep. The summary is meant to stay a few
/// hundred tokens, so the long tail is dropped (and noted when it is).
const TOP_N: usize = 8;

/// One raw transcript line, with only the fields the accounting reads. Every
/// field is optional so a record of any `type` (summary, attachment, system,
/// …) deserializes and is simply ignored when it carries no `message`.
#[derive(Deserialize)]
struct RawRecord {
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    usage: Option<Usage>,
    /// `content` is polymorphic: a plain user turn is a bare string, while
    /// assistant turns and tool results are arrays of typed items.
    content: Option<Value>,
}

/// The token-usage block on an assistant record. Defaults cover older records
/// that predate one of the cache fields.
#[derive(Deserialize, Serialize, Clone, Copy, Default)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

/// Session-wide token totals, summed across every assistant turn.
#[derive(Serialize, Clone, Copy, Default)]
pub struct Totals {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub turns: u64,
}

impl Totals {
    fn add(&mut self, u: &Usage) {
        self.input += u.input_tokens;
        self.output += u.output_tokens;
        self.cache_creation += u.cache_creation_input_tokens;
        self.cache_read += u.cache_read_input_tokens;
        self.turns += 1;
    }

    /// Total input the model processed: fresh input plus both cache tiers.
    fn total_input(&self) -> u64 {
        self.input + self.cache_creation + self.cache_read
    }
}

/// Per-tool rollup: how many times it was called and the total bytes its
/// results contributed to the transcript.
#[derive(Serialize, Default)]
pub struct ToolLine {
    pub name: String,
    pub calls: u64,
    pub result_bytes: u64,
}

/// A single largest-result entry, with a short label drawn from the call's
/// input (the file for a Read, the command for a Bash, the method for an MCP
/// call).
#[derive(Serialize)]
pub struct SinkLine {
    pub name: String,
    pub label: String,
    pub bytes: u64,
}

/// Per-sub-agent token rollup, summed from that agent's own transcript file.
#[derive(Serialize)]
pub struct SubAgentLine {
    pub agent: String,
    pub turns: u64,
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl SubAgentLine {
    fn total_input(&self) -> u64 {
        self.input + self.cache_creation + self.cache_read
    }
}

/// The compact, ranked summary the binary prints. Everything here is small and
/// safe to surface to the model; the bulky transcript stays in the binary's
/// own process.
#[derive(Serialize)]
pub struct Report {
    pub totals: Totals,
    pub cache_hit_rate: f64,
    pub tools: Vec<ToolLine>,
    pub top_sinks: Vec<SinkLine>,
    pub subagents: Vec<SubAgentLine>,
    /// Lines that failed to parse as JSON (a truncated final line, say). Surfaced
    /// so a malformed transcript doesn't masquerade as a clean one.
    pub parse_errors: u64,
    /// Distinct tools beyond [`TOP_N`] omitted from `tools`, so the table never
    /// reads as the whole story when it isn't.
    pub tools_omitted: usize,
    /// Result entries beyond [`TOP_N`] omitted from `top_sinks`.
    pub sinks_omitted: usize,
}

/// A pending tool call awaiting its result, keyed by `tool_use_id`.
struct ToolCall {
    name: String,
    label: String,
}

/// Per-sub-agent accumulator (only usage; sub-agent tool calls aren't attributed
/// to the main-session table).
#[derive(Default)]
struct SubAgentAcc {
    turns: u64,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
}

/// Streaming accumulator: feed it transcript lines one at a time (so the binary
/// never holds the whole file), then [`finish`](Self::finish) into a [`Report`].
#[derive(Default)]
pub struct SessionAggregator {
    totals: Totals,
    pending: HashMap<String, ToolCall>,
    by_tool: HashMap<String, ToolLine>,
    sinks: Vec<SinkLine>,
    subagents: HashMap<String, SubAgentAcc>,
    parse_errors: u64,
}

impl SessionAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest one line of the main session transcript.
    pub fn ingest_main_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        match serde_json::from_str::<RawRecord>(line) {
            Ok(rec) => self.ingest_main_record(rec),
            Err(_) => self.parse_errors += 1,
        }
    }

    /// Ingest one line of a sub-agent transcript, attributed to `agent`.
    pub fn ingest_subagent_line(&mut self, agent: &str, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        match serde_json::from_str::<RawRecord>(line) {
            Ok(rec) => {
                if let Some(usage) = rec.message.and_then(|m| m.usage) {
                    let acc = self.subagents.entry(agent.to_string()).or_default();
                    acc.turns += 1;
                    acc.input += usage.input_tokens;
                    acc.output += usage.output_tokens;
                    acc.cache_creation += usage.cache_creation_input_tokens;
                    acc.cache_read += usage.cache_read_input_tokens;
                }
            }
            Err(_) => self.parse_errors += 1,
        }
    }

    fn ingest_main_record(&mut self, rec: RawRecord) {
        let Some(msg) = rec.message else { return };
        if let Some(usage) = &msg.usage {
            self.totals.add(usage);
        }
        let Some(Value::Array(items)) = msg.content else {
            return;
        };
        for item in items {
            self.ingest_content_item(&item);
        }
    }

    fn ingest_content_item(&mut self, item: &Value) {
        match item.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                let (Some(id), Some(name)) = (
                    item.get("id").and_then(Value::as_str),
                    item.get("name").and_then(Value::as_str),
                ) else {
                    return;
                };
                let label = tool_label(name, item.get("input"));
                self.pending.insert(
                    id.to_string(),
                    ToolCall {
                        name: name.to_string(),
                        label,
                    },
                );
            }
            Some("tool_result") => {
                let Some(id) = item.get("tool_use_id").and_then(Value::as_str) else {
                    return;
                };
                let bytes = item.get("content").map(value_len).unwrap_or(0) as u64;
                let (name, label) = match self.pending.remove(id) {
                    Some(call) => (call.name, call.label),
                    None => ("unknown".to_string(), String::new()),
                };
                let entry = self
                    .by_tool
                    .entry(name.clone())
                    .or_insert_with(|| ToolLine {
                        name: name.clone(),
                        calls: 0,
                        result_bytes: 0,
                    });
                entry.calls += 1;
                entry.result_bytes += bytes;
                self.sinks.push(SinkLine { name, label, bytes });
            }
            _ => {}
        }
    }

    /// Rank and truncate into the final [`Report`].
    pub fn finish(self) -> Report {
        let cache_hit_rate = if self.totals.total_input() == 0 {
            0.0
        } else {
            self.totals.cache_read as f64 / self.totals.total_input() as f64
        };

        let mut tools: Vec<ToolLine> = self.by_tool.into_values().collect();
        tools.sort_by(|a, b| {
            b.result_bytes
                .cmp(&a.result_bytes)
                .then_with(|| a.name.cmp(&b.name))
        });
        let tools_omitted = tools.len().saturating_sub(TOP_N);
        tools.truncate(TOP_N);

        let mut sinks = self.sinks;
        sinks.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
        let sinks_omitted = sinks.len().saturating_sub(TOP_N);
        sinks.truncate(TOP_N);

        let mut subagents: Vec<SubAgentLine> = self
            .subagents
            .into_iter()
            .map(|(agent, acc)| SubAgentLine {
                agent,
                turns: acc.turns,
                input: acc.input,
                output: acc.output,
                cache_creation: acc.cache_creation,
                cache_read: acc.cache_read,
            })
            .collect();
        subagents.sort_by(|a, b| {
            b.total_input()
                .cmp(&a.total_input())
                .then_with(|| a.agent.cmp(&b.agent))
        });

        Report {
            totals: self.totals,
            cache_hit_rate,
            tools,
            top_sinks: sinks,
            subagents,
            parse_errors: self.parse_errors,
            tools_omitted,
            sinks_omitted,
        }
    }
}

impl Report {
    /// Render the compact Markdown summary printed by default. Kept pure (no
    /// IO) so it round-trips in tests. `session_label` is a short session id.
    pub fn to_markdown(&self, session_label: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("## Session metrics — {session_label}\n\n"));

        out.push_str(&format!(
            "**Totals**: input {} · output {} · cache-write {} · cache-read {} · {} turns\n",
            human(self.totals.input),
            human(self.totals.output),
            human(self.totals.cache_creation),
            human(self.totals.cache_read),
            self.totals.turns,
        ));
        out.push_str(&format!(
            "**Cache-hit rate**: {:.0}% (cache-read ÷ all input)\n",
            self.cache_hit_rate * 100.0,
        ));
        if self.parse_errors > 0 {
            out.push_str(&format!(
                "**Note**: {} transcript line(s) failed to parse and were skipped.\n",
                self.parse_errors,
            ));
        }

        if !self.tools.is_empty() {
            out.push_str("\n### Costliest tools (by result size, ≈tokens = bytes ÷ 4)\n\n");
            out.push_str("| tool | calls | ≈tokens |\n|---|--:|--:|\n");
            for t in &self.tools {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    t.name,
                    t.calls,
                    human(t.result_bytes / BYTES_PER_TOKEN),
                ));
            }
            if self.tools_omitted > 0 {
                out.push_str(&format!(
                    "\n_+{} more tool(s) omitted._\n",
                    self.tools_omitted
                ));
            }
        }

        if !self.top_sinks.is_empty() {
            out.push_str("\n### Largest single results (≈tokens)\n\n");
            for (i, s) in self.top_sinks.iter().enumerate() {
                let label = if s.label.is_empty() {
                    String::new()
                } else {
                    format!("  `{}`", s.label)
                };
                out.push_str(&format!(
                    "{}. ≈{}  {}{}\n",
                    i + 1,
                    human(s.bytes / BYTES_PER_TOKEN),
                    s.name,
                    label,
                ));
            }
            if self.sinks_omitted > 0 {
                out.push_str(&format!(
                    "\n_+{} more result(s) omitted._\n",
                    self.sinks_omitted
                ));
            }
        }

        if !self.subagents.is_empty() {
            out.push_str(&format!("\n### Sub-agents ({})\n\n", self.subagents.len()));
            out.push_str("| agent | turns | ≈input | output |\n|---|--:|--:|--:|\n");
            for a in &self.subagents {
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    a.agent,
                    a.turns,
                    human(a.total_input()),
                    human(a.output),
                ));
            }
        }

        out
    }
}

/// Format a token count compactly: `1.2k`, `3.4M`, or the bare number.
fn human(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Serialized length of a tool result's `content`. A bare string is measured
/// directly; an array of content blocks (or any other shape) is measured by its
/// JSON serialization — an approximation, which is all sink *ranking* needs.
fn value_len(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len(),
        other => serde_json::to_string(other).map(|s| s.len()).unwrap_or(0),
    }
}

/// A short, human-readable label for a tool call, drawn from the field of its
/// input that identifies the work: the path for file tools, the command for
/// Bash, the method/query for an MCP call. Paths keep their tail (the filename);
/// everything else keeps its head (the command verb).
fn tool_label(name: &str, input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let pick = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| input.get(k).and_then(Value::as_str))
            .map(str::to_string)
    };
    let is_path_tool = matches!(name, "Read" | "Edit" | "Write" | "NotebookEdit");
    if is_path_tool {
        return pick(&["file_path", "notebook_path"])
            .map(|p| shorten_tail(&p))
            .unwrap_or_default();
    }
    if name == "Bash" {
        return pick(&["command"])
            .map(|c| shorten_head(&c))
            .unwrap_or_default();
    }
    if name.starts_with("mcp__") {
        return pick(&["method", "query", "id", "pullNumber"])
            .map(|m| shorten_head(&m))
            .unwrap_or_default();
    }
    pick(&[
        "file_path",
        "command",
        "pattern",
        "query",
        "url",
        "description",
    ])
    .map(|v| shorten_head(&v))
    .unwrap_or_default()
}

/// Maximum label width before truncation.
const LABEL_WIDTH: usize = 56;

/// Keep the head of a value, marking truncation with a trailing ellipsis.
fn shorten_head(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= LABEL_WIDTH {
        return s;
    }
    let head: String = s.chars().take(LABEL_WIDTH - 1).collect();
    format!("{head}…")
}

/// Keep the tail of a value (the filename of a path), marking truncation with a
/// leading ellipsis.
fn shorten_tail(s: &str) -> String {
    if s.chars().count() <= LABEL_WIDTH {
        return s.to_string();
    }
    let skip = s.chars().count() - (LABEL_WIDTH - 1);
    let tail: String = s.chars().skip(skip).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compact assistant record with one usage block and any number of
    /// tool_use items, as a JSON line.
    fn assistant(usage: &str, tool_uses: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","usage":{usage},"content":[{tool_uses}]}}}}"#
        )
    }

    fn tool_use(id: &str, name: &str, input: &str) -> String {
        format!(r#"{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}"#)
    }

    /// A user record carrying one tool_result, as a JSON line.
    fn tool_result(id: &str, content: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":{content}}}]}}}}"#
        )
    }

    #[test]
    fn sums_usage_across_turns() {
        let mut agg = SessionAggregator::new();
        agg.ingest_main_line(&assistant(
            r#"{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":200,"cache_read_input_tokens":700}"#,
            "",
        ));
        agg.ingest_main_line(&assistant(
            r#"{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":300}"#,
            "",
        ));
        let report = agg.finish();
        assert_eq!(report.totals.input, 110);
        assert_eq!(report.totals.output, 55);
        assert_eq!(report.totals.cache_creation, 200);
        assert_eq!(report.totals.cache_read, 1000);
        assert_eq!(report.totals.turns, 2);
        // cache_read / (input + cache_creation + cache_read) = 1000 / 1310
        assert!((report.cache_hit_rate - 1000.0 / 1310.0).abs() < 1e-9);
    }

    #[test]
    fn attributes_results_to_their_tool() {
        let mut agg = SessionAggregator::new();
        agg.ingest_main_line(&assistant(
            r#"{"output_tokens":1}"#,
            &format!(
                "{},{}",
                tool_use("t1", "Read", r#"{"file_path":"/a/b/fixture.rs"}"#),
                tool_use("t2", "Bash", r#"{"command":"cargo test -p dropset-tui"}"#),
            ),
        ));
        // 40-byte result for the Read, 4-byte for the Bash.
        agg.ingest_main_line(&tool_result(
            "t1",
            r#""0123456789012345678901234567890123456789""#,
        ));
        agg.ingest_main_line(&tool_result("t2", r#""abcd""#));
        let report = agg.finish();

        let read = report.tools.iter().find(|t| t.name == "Read").unwrap();
        assert_eq!(read.calls, 1);
        assert_eq!(read.result_bytes, 40);
        // Ranked: Read (40 bytes) is the largest sink, then Bash (4).
        assert_eq!(report.top_sinks[0].name, "Read");
        assert_eq!(report.top_sinks[0].bytes, 40);
        assert_eq!(report.top_sinks[0].label, "/a/b/fixture.rs");
        assert_eq!(report.top_sinks[1].name, "Bash");
        assert_eq!(report.top_sinks[1].label, "cargo test -p dropset-tui");
    }

    #[test]
    fn unmatched_result_falls_back_to_unknown() {
        let mut agg = SessionAggregator::new();
        agg.ingest_main_line(&tool_result("orphan", r#""data""#));
        let report = agg.finish();
        assert_eq!(report.tools[0].name, "unknown");
        assert_eq!(report.tools[0].calls, 1);
    }

    #[test]
    fn array_content_result_is_measured_by_serialization() {
        let mut agg = SessionAggregator::new();
        agg.ingest_main_line(&assistant(
            r#"{"output_tokens":1}"#,
            &tool_use("t1", "Grep", r#"{"pattern":"foo"}"#),
        ));
        agg.ingest_main_line(&tool_result("t1", r#"[{"type":"text","text":"a result"}]"#));
        let report = agg.finish();
        let grep = report.tools.iter().find(|t| t.name == "Grep").unwrap();
        assert!(grep.result_bytes > 0);
    }

    #[test]
    fn subagent_usage_rolls_up_per_agent() {
        let mut agg = SessionAggregator::new();
        agg.ingest_subagent_line(
            "agent-explore",
            &assistant(
                r#"{"input_tokens":5000,"output_tokens":300,"cache_read_input_tokens":1000}"#,
                "",
            ),
        );
        agg.ingest_subagent_line(
            "agent-explore",
            &assistant(r#"{"input_tokens":100,"output_tokens":20}"#, ""),
        );
        let report = agg.finish();
        assert_eq!(report.subagents.len(), 1);
        let a = &report.subagents[0];
        assert_eq!(a.agent, "agent-explore");
        assert_eq!(a.turns, 2);
        assert_eq!(a.input, 5100);
        assert_eq!(a.output, 320);
        assert_eq!(a.cache_read, 1000);
        // Sub-agent tool calls are not attributed to the main tool table.
        assert!(report.tools.is_empty());
    }

    #[test]
    fn malformed_lines_are_counted_not_fatal() {
        let mut agg = SessionAggregator::new();
        agg.ingest_main_line("{not valid json");
        agg.ingest_main_line("");
        agg.ingest_main_line(&assistant(r#"{"output_tokens":7}"#, ""));
        let report = agg.finish();
        assert_eq!(report.parse_errors, 1); // blank line is skipped, not an error
        assert_eq!(report.totals.output, 7);
    }

    #[test]
    fn non_message_records_are_ignored() {
        let mut agg = SessionAggregator::new();
        agg.ingest_main_line(r#"{"type":"summary","summary":"a title"}"#);
        agg.ingest_main_line(r#"{"type":"attachment","attachment":{"type":"skill_listing"}}"#);
        let report = agg.finish();
        assert_eq!(report.totals.turns, 0);
        assert_eq!(report.parse_errors, 0);
    }
}
