#!/usr/bin/env python3
"""
parse_trace.py — Multi-format parser for agentic coding session traces.

Supports two trace formats:
  - **Claude Code** (default): mixed plain-text + JSON Lines with
    assistant/user message records, sub-agent task brackets, and
    per-model usage summaries.
  - **OpenCode** (`--format opencode`): flat JSONL event stream with
    `step_start`, `reasoning`, `text`, `tool_use`, and `step_finish`
    events.  Token/cost totals arrive per-step rather than per-session.

Auto-detection (`--format auto`, the default) peeks at the first few
JSON records and chooses the right parser automatically.

Usage:
    python3 parse_trace.py trace.txt              # auto-detect
    python3 parse_trace.py trace.txt --format claude
    python3 parse_trace.py trace.txt -r -v        # readable + SVG
"""

from __future__ import annotations

import html
import json
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from dataclasses import dataclass, field
from typing import Literal, Optional


# ---------------------------------------------------------------------------
# Data Structures
# ---------------------------------------------------------------------------

@dataclass
class TokenUsage:
    """Agent-agnostic token counts for one API call / step.

    `output_tokens` always includes reasoning/thinking tokens, so it can be
    multiplied by the model's output price directly. Claude already reports
    them that way; the OpenCode parsers fold `tokens.reasoning` in at
    ingestion. `reasoning_tokens` is the informational subset, NOT an
    addition — never sum it on top of `output_tokens`.
    """
    input_tokens: int
    output_tokens: int
    cache_creation_tokens: int
    cache_read_tokens: int
    reasoning_tokens: int = 0


@dataclass
class ProgressSnapshot:
    """
    A framework-emitted progress update for a running sub-agent.
    These are emitted as system/task_progress events and do NOT enter
    the Claude API conversation — they are observability artifacts only.
    """
    total_tokens: int
    tool_uses: int
    duration_ms: int
    last_tool_name: str
    description: str


@dataclass
class ToolResult:
    tool_use_id: str   # matches the ToolUse.id that produced this result
    content: str
    is_error: bool = False


@dataclass
class SubAgent:
    """
    A sub-agent invocation embedded inside an Agent tool call.

    SYNC vs ASYNC:
    - Synchronous (default; `run_in_background` not set or false): the parent's
      API call loop blocks until this SubAgent finishes and returns its result.
      Its full conversation is recorded in the trace as assistant/user records
      tagged with `parent_tool_use_id` — so `conversation` is fully populated.
    - Asynchronous (`run_in_background: true`): the parent receives an
      "Async agent launched" tool_result immediately and continues working.
      The async agent's actual conversation is NOT in the trace; only
      `task_progress` snapshots are emitted (one per tool call inside it)
      describing what it was doing. For these, `conversation` is synthesized
      from `progress_snapshots` so the visualizer has something to show.

    The sub-agent's execution is a recursive list[Turn], embedded here rather
    than at the Session level. This tree naturally handles deeper nesting
    (sub-agents spawning their own sub-agents) without schema changes.

    Linkage: SubAgent.tool_use_id == the parent ToolUse.id that spawned it.
    This is the same ID that appears in system/task_started,
    system/task_progress, system/task_notification, and the final user
    tool_result that returns control to the parent.
    """
    task_id: str
    tool_use_id: str
    description: str
    prompt: str
    status: str

    # Full nested conversation of the sub-agent (same structure as parent).
    # For async sub-agents this is synthesized from progress_snapshots and
    # is_async is set True.
    conversation: list[Turn] = field(default_factory=list)
    is_async: bool = False

    # Context compactions that occurred inside this sub-agent session.
    compact_events: list[CompactEvent] = field(default_factory=list)

    # Framework-level progress snapshots (not API messages)
    progress_snapshots: list[ProgressSnapshot] = field(default_factory=list)

    # Aggregate stats from system/task_notification
    total_tokens: int = 0
    total_tool_uses: int = 0
    duration_ms: int = 0


@dataclass
class ToolUse:
    """
    One tool invocation within a Turn.

    PARALLEL EXECUTION: Multiple ToolUse objects may exist within a single
    Turn. When the model outputs multiple tool_use blocks in one API
    response, Claude Code executes them concurrently on the local machine.
    However, the Turn boundary is strictly synchronous: ALL ToolUse objects
    in a Turn must complete before the next Turn's API request is sent.

    Matching: ToolUse.id (a.k.a. tool_use_id) is the join key between a
    tool_use content block and its tool_result. Matching is by ID, not by
    position, so parallel results that arrive out of order are handled
    correctly.

    Sub-agent case: when name == "Agent", this ToolUse spawns a sub-agent.
    The subtask field holds the full nested execution. The parent turn does
    not advance until subtask completes and result is populated.
    """
    id: str       # tool_use_id
    name: str     # e.g. "Bash", "Read", "Write", "Agent", "Edit"
    input: dict

    # Populated after local execution (or sub-agent completion)
    result: Optional[ToolResult] = None

    # Populated only when name == "Agent"
    subagent: Optional[SubAgent] = None

    # When set, overrides the size computed from `result.content` etc.
    # Used for synthesized ToolUses (async sub-agent progress events) where
    # the real result content isn't in the trace; we substitute a token-delta
    # estimate so the bar segment has a meaningful width.
    size_override: Optional[int] = None

    # True when the trace shows this call never completed: an OpenCode export
    # captured its state as "running"/"pending", meaning the agent process
    # ended (typically killed by the harness timeout) while the tool — most
    # often a `task` sub-agent — was still in flight.
    frozen: bool = False


@dataclass
class ContentBlock:
    """
    One content block within an API response, in server emission order.

    Within a single Turn, the server streams blocks in a fixed order:
        thinking* → text* → tool_use*

    Claude API enforces that all thinking blocks precede any tool_use block.
    Preserving this ordering allows downstream analysis of the model's
    reasoning-before-action pattern.

    Each ContentBlock corresponds to one JSON record in the trace file
    (Claude Code emits each block as a separate streaming event), but all
    blocks in a Turn share the same logical API response.
    """
    type: Literal["thinking", "text", "tool_use"]
    thinking: Optional[str] = None
    text: Optional[str] = None
    tool_use: Optional[ToolUse] = None


@dataclass
class Turn:
    """
    One complete API request-response cycle for a given agent.

    Exactly one HTTP request is sent to the Claude API per Turn; the
    response may be streamed as multiple JSON events (one per ContentBlock)
    but they are all part of the same Turn.

    SYNCHRONOUS BARRIER: A new Turn does not begin until every ToolUse
    from the previous Turn has completed and returned a result. This
    includes Agent tool calls (sub-agents), which may internally span
    many turns of their own. The next API request carries the full
    accumulated context: all prior content + all tool results.

    turn_index counts only the turns of the agent that owns this
    conversation. Sub-agent turns (nested inside a SubAgent) are counted
    separately in their own conversation and never contribute to the
    parent's index. This matches the num_turns field in the result event.
    """
    turn_index: int
    content_blocks: list[ContentBlock] = field(default_factory=list)

    # Usage from the last streaming event of this turn's assistant records
    # (Claude Code echoes usage on each streamed block; only the final
    # value is meaningful as the cumulative total for the API call)
    usage: Optional[TokenUsage] = None

    @property
    def tool_uses(self) -> list[ToolUse]:
        return [b.tool_use for b in self.content_blocks
                if b.type == "tool_use" and b.tool_use is not None]

    @property
    def thinking_texts(self) -> list[str]:
        return [b.thinking for b in self.content_blocks
                if b.type == "thinking" and b.thinking is not None]

    @property
    def texts(self) -> list[str]:
        return [b.text for b in self.content_blocks
                if b.type == "text" and b.text is not None]

    @property
    def is_final(self) -> bool:
        """True if this turn produced no tool calls (conversation end)."""
        return len(self.tool_uses) == 0


@dataclass
class MonitoringEvent:
    """
    A framework-level event that does NOT enter the Claude API conversation.

    system/init, system/task_started, system/task_progress,
    system/task_notification, and rate_limit_event records are emitted by
    the Claude Code framework for observability. They share the session_id
    but are never included in API requests. Keeping them in a separate list
    makes it easy to reconstruct the pure API conversation history.
    """
    type: str
    subtype: Optional[str]
    line_number: int
    raw: dict


@dataclass
class CompactEvent:
    """
    A context-window compaction (`system/compact_boundary` event).

    Claude Code summarizes the prior conversation when context approaches the
    model's limit. The next API request then carries this compressed summary
    instead of the full history — ~95% smaller in our traces. Mark these as
    discontinuities: facts the agent established before a compact may be
    represented only fuzzily afterwards.
    """
    pre_tokens: int       # context size before compaction (tokens)
    post_tokens: int      # context size after compaction (tokens)
    duration_ms: int      # time the compaction itself took
    trigger: str          # "auto" or "manual"
    line_number: int

    # 1-based turn index AFTER which this compaction occurred. 0 means
    # "before any main-conversation turns". Populated post-parse by counting
    # main-conversation turn boundaries with line numbers below this event.
    after_turn_index: int = 0


@dataclass
class InitEvent:
    model: str
    cwd: str
    permission_mode: str
    claude_code_version: str
    tool_names: list[str]


@dataclass
class ModelUsage:
    """Per-model session totals. `output_tokens` includes reasoning tokens
    (see TokenUsage); `reasoning_tokens` is the informational subset."""
    model: str
    input_tokens: int
    output_tokens: int
    cache_read_tokens: int
    cache_creation_tokens: int
    cost_usd: float
    reasoning_tokens: int = 0


@dataclass
class ResultEvent:
    is_error: bool
    stop_reason: str
    num_turns: int
    duration_ms: int
    duration_api_ms: int
    total_cost_usd: float
    result_text: str
    model_usage: dict[str, ModelUsage]  # keyed by model name


@dataclass
class Session:
    """
    One complete agent session lifecycle.

    A Session spans from system/init to the result event. Every JSON record
    sharing the same session_id belongs to one Session.

    SEPARATION OF CONCERNS:
    - conversation: only the top-level agent's Turns (pure API history)
    - monitoring:   framework events that never entered the API
    Sub-agent turns live recursively inside ToolUse.subtask.conversation,
    not in this list. This preserves the tree structure of nested agents
    while keeping the top-level view clean.
    """
    session_id: str
    phase: str   # "translation" | "verification" | "unknown"
    agent_type: str = "claude"  # "claude" | "opencode"
    data_source: str = "jsonl"  # "export" | "live-export" | "jsonl" | "sqlite"

    init: Optional[InitEvent] = None
    conversation: list[Turn] = field(default_factory=list)
    monitoring: list[MonitoringEvent] = field(default_factory=list)
    compact_events: list[CompactEvent] = field(default_factory=list)
    result: Optional[ResultEvent] = None

    # Wall-clock duration in ms, derived from the difference between the
    # earliest and latest event `timestamp` for this session_id. This is the
    # only field that matches "real time elapsed". The framework's
    # `result.duration_ms` is broken for async-heavy sessions (only counts
    # main-agent-active time), and `result.duration_api_ms` double-counts
    # parallel sub-agent work.
    wall_clock_ms: int = 0

    # Wall-clock duration of the agent *process* (from the harness's
    # "Invoking ... agent" log line to its post-exit export/append line),
    # when those agent_runner markers are present in the trace. A large gap
    # between this and `wall_clock_ms` means the process sat idle after the
    # session's last recorded activity — e.g. an OpenCode stall that ended
    # only when the harness timeout killed the process.
    process_wall_ms: int = 0


# ---------------------------------------------------------------------------
# Parser
# ---------------------------------------------------------------------------

def _wall_clock_ms(events: list[tuple[int, dict]]) -> int:
    """Span between the earliest and latest event `timestamp` across `events`.
    Handles both ISO-8601 strings (Claude Code) and epoch-ms integers (OpenCode).
    Returns 0 if no timestamps are available."""
    from datetime import datetime
    first_ts = None
    last_ts = None
    for _, obj in events:
        ts_raw = obj.get("timestamp")
        if ts_raw is None:
            continue
        # OpenCode emits epoch-ms as int; Claude emits ISO strings.
        if isinstance(ts_raw, (int, float)):
            t = datetime.fromtimestamp(ts_raw / 1000.0)
        elif isinstance(ts_raw, str):
            try:
                t = datetime.fromisoformat(ts_raw.replace("Z", "+00:00"))
            except (TypeError, ValueError):
                continue
        else:
            continue
        if first_ts is None or t < first_ts:
            first_ts = t
        if last_ts is None or t > last_ts:
            last_ts = t
    if first_ts is None or last_ts is None:
        return 0
    return int((last_ts - first_ts).total_seconds() * 1000)


def _extract_tool_result_content(raw_content) -> str:
    if isinstance(raw_content, str):
        return raw_content
    if isinstance(raw_content, list):
        return "\n".join(
            item.get("text", "") if isinstance(item, dict) else str(item)
            for item in raw_content
        )
    return str(raw_content)


def _parse_token_usage(raw: dict) -> TokenUsage:
    return TokenUsage(
        input_tokens=raw.get("input_tokens", 0),
        output_tokens=raw.get("output_tokens", 0),
        cache_creation_tokens=raw.get("cache_creation_input_tokens", 0),
        cache_read_tokens=raw.get("cache_read_input_tokens", 0),
        reasoning_tokens=raw.get("reasoning_tokens", 0),
    )


class TraceParser:
    def parse_file(self, path: str) -> list[Session]:
        """Read the trace file and return one Session per session_id."""
        records: list[tuple[int, dict]] = []
        with open(path, "r") as f:
            for lineno, line in enumerate(f, 1):
                stripped = line.strip()
                if stripped.startswith("{"):
                    try:
                        records.append((lineno, json.loads(stripped)))
                    except json.JSONDecodeError:
                        pass

        by_session: dict[str, list[tuple[int, dict]]] = defaultdict(list)
        for lineno, obj in records:
            sid = obj.get("session_id", "unknown")
            by_session[sid].append((lineno, obj))

        sessions = []
        for idx, (sid, events) in enumerate(by_session.items()):
            phases = ["translation", "verification", "unknown"]
            phase = phases[min(idx, 2)]
            sessions.append(self._parse_session(sid, phase, events))
        return sessions

    def _parse_session(
        self, session_id: str, phase: str, events: list[tuple[int, dict]]
    ) -> Session:
        session = Session(session_id=session_id, phase=phase)

        # ------------------------------------------------------------------
        # Step 1: Identify sub-agent brackets.
        #
        # A sub-agent bracket is the range of event indices [start+1, end)
        # between a system/task_started and its matching task_notification,
        # both carrying the same tool_use_id.
        # Records inside a bracket belong to the sub-agent's conversation.
        # ------------------------------------------------------------------
        subagent_map: dict[str, SubAgent] = {}   # tool_use_id -> SubAgent
        task_stack: dict[str, int] = {}        # tool_use_id -> event index
        task_brackets: dict[str, tuple[int, int]] = {}  # tool_use_id -> (start, end)
        workflow_tasks: dict[str, str] = {}    # task_id -> tool_use_id
        workflow_progress: dict[str, list[dict]] = defaultdict(list)  # tool_use_id -> progress events

        for i, (lineno, obj) in enumerate(events):
            sub = obj.get("subtype")
            if sub == "task_started":
                tid = obj.get("tool_use_id", "")
                task_id = obj.get("task_id", "")
                if tid:
                    if obj.get("task_type") == "local_workflow":
                        # Workflow: don't open a bracket (no task_notification).
                        # Collect progress events and synthesize later.
                        workflow_tasks[task_id] = tid
                        subagent_map[tid] = SubAgent(
                            task_id=task_id,
                            tool_use_id=tid,
                            description=obj.get("description", ""),
                            prompt=obj.get("prompt", ""),
                            status="running",
                        )
                    else:
                        task_stack[tid] = i
                        subagent_map[tid] = SubAgent(
                            task_id=task_id,
                            tool_use_id=tid,
                            description=obj.get("description", ""),
                            prompt=obj.get("prompt", ""),
                            status="running",
                        )
            elif sub == "task_notification":
                tid = obj.get("tool_use_id", "")
                if tid and tid in task_stack:
                    start_idx = task_stack.pop(tid)
                    task_brackets[tid] = (start_idx, i)
                    st = subagent_map[tid]
                    st.status = obj.get("status", "completed")
                    usage = obj.get("usage", {})
                    st.total_tokens = usage.get("total_tokens", 0)
                    st.total_tool_uses = usage.get("tool_uses", 0)
                    st.duration_ms = usage.get("duration_ms", 0)
            elif sub == "task_progress":
                tid = obj.get("tool_use_id", "")
                task_id = obj.get("task_id", "")
                # Route workflow progress separately from regular sub-agent progress.
                if task_id in workflow_tasks:
                    workflow_progress[workflow_tasks[task_id]].append(obj)
                elif tid and tid in subagent_map:
                    usage = obj.get("usage", {})
                    subagent_map[tid].progress_snapshots.append(ProgressSnapshot(
                        total_tokens=usage.get("total_tokens", 0),
                        tool_uses=usage.get("tool_uses", 0),
                        duration_ms=usage.get("duration_ms", 0),
                        last_tool_name=obj.get("last_tool_name", "") or "",
                        description=obj.get("description", "") or "",
                    ))
            elif sub == "task_updated":
                task_id = obj.get("task_id", "")
                if task_id in workflow_tasks:
                    tid = workflow_tasks[task_id]
                    if tid in subagent_map:
                        patch = obj.get("patch", {})
                        if patch.get("status"):
                            subagent_map[tid].status = patch["status"]

        # Map each event index to the sub-agent tool_use_id it belongs to
        subagent_index: dict[int, str] = {}
        for tid, (start, end) in task_brackets.items():
            for i in range(start + 1, end):
                subagent_index[i] = tid

        # ------------------------------------------------------------------
        # Step 2: Route each event to main conversation, sub-agent bucket,
        # or monitoring.
        # ------------------------------------------------------------------
        main_records: list[tuple[int, dict]] = []
        subagent_records: dict[str, list[tuple[int, dict]]] = defaultdict(list)

        for i, (lineno, obj) in enumerate(events):
            typ = obj.get("type")
            sub = obj.get("subtype")

            if typ in ("system", "rate_limit_event"):
                session.monitoring.append(MonitoringEvent(
                    type=typ, subtype=sub, line_number=lineno, raw=obj,
                ))
                if sub == "init":
                    session.init = self._parse_init(obj)
                elif sub == "compact_boundary" and i not in subagent_index:
                    md = obj.get("compact_metadata") or {}
                    session.compact_events.append(CompactEvent(
                        pre_tokens=md.get("pre_tokens", 0),
                        post_tokens=md.get("post_tokens", 0),
                        duration_ms=md.get("duration_ms", 0),
                        trigger=md.get("trigger", "") or "",
                        line_number=lineno,
                    ))
                continue

            if typ == "result":
                session.result = self._parse_result(obj)
                continue

            if typ not in ("assistant", "user"):
                continue

            # Route by parent_tool_use_id, NOT by task_brackets index ranges.
            # When the main agent dispatches a second sub-agent before the first
            # one finishes (e.g. it doesn't strictly wait), the brackets overlap
            # and `subagent_index[i] = tid` becomes last-write-wins, mis-routing
            # any record (including the parent's own tool_result) that falls in
            # the overlap. parent_tool_use_id is the authoritative pointer:
            # main-agent records have it empty, sub-agent records carry their
            # parent's tool_use_id. See c17 T17/T19 for the failure mode.
            parent_tid = obj.get("parent_tool_use_id") or ""
            if parent_tid:
                subagent_records[parent_tid].append((lineno, obj))
                # Be safe: a parent_tool_use_id we haven't seen via task_started
                # would otherwise have no SubAgent metadata. Create a stub.
                if parent_tid not in subagent_map:
                    subagent_map[parent_tid] = SubAgent(
                        task_id="", tool_use_id=parent_tid,
                        description="", prompt="", status="unknown",
                    )
            else:
                main_records.append((lineno, obj))

        # ------------------------------------------------------------------
        # Step 3: Build conversations.
        # ------------------------------------------------------------------
        session.conversation = self._parse_conversation(main_records)

        # Locate each compact event in the main turn sequence by counting how
        # many turn boundaries fall before its line number. A turn boundary
        # is an assistant record that does not immediately follow another
        # assistant record (consecutive assistant records are streamed chunks
        # of one API response).
        turn_start_linenos: list[int] = []
        prev_was_assistant = False
        for lineno, obj in main_records:
            is_assistant = obj.get("type") == "assistant"
            if is_assistant and not prev_was_assistant:
                turn_start_linenos.append(lineno)
            prev_was_assistant = is_assistant
        for ev in session.compact_events:
            ev.after_turn_index = sum(
                1 for ts in turn_start_linenos if ts < ev.line_number
            )

        for tid, records in subagent_records.items():
            if tid in subagent_map:
                subagent_map[tid].conversation = self._parse_conversation(records)

        # Async sub-agents (`run_in_background: true`) leave no parented
        # assistant/user records — the trace only carries `task_progress`
        # snapshots. For each such sub-agent, synthesize an approximate turn
        # list from its progress snapshots so the visualizer can show the
        # internal activity (one row per progress snapshot).
        for tid, sa in subagent_map.items():
            if not sa.conversation and sa.progress_snapshots:
                sa.is_async = True
                sa.conversation = _synthesize_async_subagent_turns(
                    sa.progress_snapshots
                )

        # Synthesize workflow sub-agents from task_progress events.
        # Workflow phases are independent context-isolated agents.  Create one
        # SubAgent per phase (agent label), each with grey turns (we don't know
        # the individual tool calls).  Inject synthetic Task ToolUses after the
        # original Workflow ToolUse so each phase gets its own purple guide line.
        # The original Workflow ToolUse stays for the purple dispatch bar on T1.
        for tid, progress_events in workflow_progress.items():
            if tid not in subagent_map:
                continue
            parent_sa = subagent_map[tid]
            # Inherit description/prompt from the task_started event so tooltips
            # on the purple bars show meaningful content.
            parent_desc = parent_sa.description or ""
            parent_prompt = parent_sa.prompt or ""

            # Group progress events by agent label.
            agent_groups: dict[str, list[dict]] = defaultdict(list)
            agent_order: list[str] = []
            for ev in progress_events:
                desc = ev.get("description", "") or "workflow-step"
                if desc not in agent_groups:
                    agent_order.append(desc)
                agent_groups[desc].append(ev)

            # Create one SubAgent per workflow agent.
            # prev_tokens carries across agents because task_progress total_tokens
            # is cumulative across the entire workflow, not per-agent.
            wf_subagents: list[tuple[str, SubAgent]] = []
            prev_tokens = 0
            for label in agent_order:
                evts = agent_groups[label]
                turns, prev_tokens = _synthesize_workflow_agent_turns(evts, prev_tokens)
                # Use per-agent tokens from workflow_progress[].tokens (which is
                # cumulative per-agent, starting from ~8k for the system prompt).
                # Do NOT use event-level usage.total_tokens — that is cumulative
                # across the ENTIRE workflow and would double-count.
                # Note: wp.label (e.g. "setup") differs from event description
                # (e.g. "Setup: setup"), so we match by taking the LAST
                # workflow_agent entry's tokens (each event has one active agent).
                agent_token_end = 0
                for ev in evts:
                    for wp in ev.get("workflow_progress", []):
                        if wp.get("type") == "workflow_agent":
                            tok = wp.get("tokens", 0)
                            if tok:
                                agent_token_end = tok
                last_usage = evts[-1].get("usage", {}) if evts else {}
                sa = SubAgent(
                    task_id=parent_sa.task_id,
                    tool_use_id=f"{tid}:{label}",
                    description=label,
                    prompt=parent_prompt,
                    status="completed",
                    conversation=turns,
                    is_async=True,
                    total_tokens=agent_token_end,
                    total_tool_uses=last_usage.get("tool_uses", 0),
                    duration_ms=last_usage.get("duration_ms", 0),
                )
                subagent_map[sa.tool_use_id] = sa
                wf_subagents.append((label, sa))

            # Inject synthetic Task ToolUses into the parent turn after the
            # original Workflow ToolUse.  Each Task carries its own SubAgent.
            # Also patch the Workflow ToolUse input so its tooltip shows
            # the description and prompt (original input only has "script").
            # Then remove the parent SubAgent so it doesn't create a phantom
            # empty sub-agent block with 0 turns.
            for turn in session.conversation:
                for blk in turn.content_blocks:
                    if blk.type == "tool_use" and blk.tool_use and blk.tool_use.id == tid:
                        # Patch Workflow ToolUse input for tooltip.
                        if isinstance(blk.tool_use.input, dict):
                            blk.tool_use.input.setdefault("description", parent_desc)
                            blk.tool_use.input.setdefault("prompt", parent_prompt)
                        insert_idx = turn.content_blocks.index(blk) + 1
                        for j, (label, sa) in enumerate(wf_subagents):
                            synthetic_tu = ToolUse(
                                id=sa.tool_use_id,
                                name="Task",
                                input={
                                    "description": label,
                                    "prompt": parent_prompt,
                                },
                            )
                            synthetic_tu.subagent = sa
                            turn.content_blocks.insert(
                                insert_idx + j,
                                ContentBlock(type="tool_use", tool_use=synthetic_tu),
                            )
                        break
            subagent_map.pop(tid, None)

        # Attach SubAgent objects to their parent Agent ToolUse
        self._attach_subagents(session.conversation, subagent_map)

        # Compute true wall-clock duration: span between earliest and latest
        # `timestamp` field across all events for this session_id. Robust
        # against async sub-agents (frame's duration_ms misses idle time)
        # and against parallelism (frame's duration_api_ms double-counts).
        session.wall_clock_ms = _wall_clock_ms(events)

        return session

    def _parse_conversation(self, records: list[tuple[int, dict]]) -> list[Turn]:
        """
        Build a Turn list from a flat sequence of assistant/user records.

        A new Turn begins at each assistant record that follows a non-
        assistant record. Consecutive assistant records belong to the same
        Turn (they are streaming chunks of one API response).

        Tool results are matched to ToolUse by tool_use_id, not by position,
        so parallel tool calls with out-of-order results are handled correctly.
        """
        turns: list[Turn] = []
        i = 0

        while i < len(records):
            _, obj = records[i]
            if obj.get("type") != "assistant":
                i += 1
                continue

            turn = Turn(turn_index=len(turns) + 1)
            pending: dict[str, ToolUse] = {}   # tool_use_id -> ToolUse
            last_usage: Optional[TokenUsage] = None

            # Collect consecutive assistant records (one API response)
            while i < len(records) and records[i][1].get("type") == "assistant":
                msg = records[i][1].get("message", {})
                raw_usage = msg.get("usage")
                if raw_usage:
                    last_usage = _parse_token_usage(raw_usage)
                for block in msg.get("content", []):
                    btype = block.get("type")
                    if btype == "thinking":
                        turn.content_blocks.append(ContentBlock(
                            type="thinking", thinking=block.get("thinking", ""),
                        ))
                    elif btype == "text":
                        turn.content_blocks.append(ContentBlock(
                            type="text", text=block.get("text", ""),
                        ))
                    elif btype == "tool_use":
                        tu = ToolUse(
                            id=block.get("id", ""),
                            name=block.get("name", ""),
                            input=block.get("input", {}),
                        )
                        pending[tu.id] = tu
                        turn.content_blocks.append(ContentBlock(
                            type="tool_use", tool_use=tu,
                        ))
                i += 1

            turn.usage = last_usage

            # Collect following user records (tool results for this turn)
            while i < len(records) and records[i][1].get("type") == "user":
                msg = records[i][1].get("message", {})
                for block in msg.get("content", []):
                    if block.get("type") == "tool_result":
                        tid = block.get("tool_use_id", "")
                        result = ToolResult(
                            tool_use_id=tid,
                            content=_extract_tool_result_content(block.get("content", "")),
                            is_error=block.get("is_error", False),
                        )
                        if tid in pending:
                            pending[tid].result = result
                i += 1

            turns.append(turn)

        return turns

    def _attach_subagents(
        self, turns: list[Turn], subagent_map: dict[str, SubAgent]
    ) -> None:
        """Wire SubAgent objects into their parent Agent ToolUse nodes."""
        for turn in turns:
            for block in turn.content_blocks:
                if block.type == "tool_use" and block.tool_use is not None:
                    tu = block.tool_use
                    if tu.name.lower() in ("agent", "task", "workflow") and tu.id in subagent_map:
                        tu.subagent = subagent_map[tu.id]

    def _parse_init(self, obj: dict) -> InitEvent:
        tools = obj.get("tools", [])
        tool_names = [
            t.get("name", "") if isinstance(t, dict) else str(t) for t in tools
        ]
        return InitEvent(
            model=obj.get("model", ""),
            cwd=obj.get("cwd", ""),
            permission_mode=obj.get("permissionMode", ""),
            claude_code_version=obj.get("claude_code_version", ""),
            tool_names=tool_names,
        )

    def _parse_result(self, obj: dict) -> ResultEvent:
        model_usage = {}
        for name, mu in obj.get("modelUsage", {}).items():
            model_usage[name] = ModelUsage(
                model=name,
                input_tokens=mu.get("inputTokens", 0),
                output_tokens=mu.get("outputTokens", 0),
                cache_read_tokens=mu.get("cacheReadInputTokens", 0),
                cache_creation_tokens=mu.get("cacheCreationInputTokens", 0),
                cost_usd=mu.get("costUSD", 0.0),
            )
        return ResultEvent(
            is_error=obj.get("is_error", False),
            stop_reason=obj.get("stop_reason", ""),
            num_turns=obj.get("num_turns", 0),
            duration_ms=obj.get("duration_ms", 0),
            duration_api_ms=obj.get("duration_api_ms", 0),
            total_cost_usd=obj.get("total_cost_usd", 0.0),
            result_text=obj.get("result", ""),
            model_usage=model_usage,
        )


# ---------------------------------------------------------------------------
# Validation / Statistics
# ---------------------------------------------------------------------------

def count_tools_in_conversation(turns: list[Turn]) -> dict[str, int]:
    counts: dict[str, int] = defaultdict(int)
    for turn in turns:
        for tu in turn.tool_uses:
            counts[tu.name] += 1
    return dict(counts)


def print_session_stats(sessions: list[Session]) -> None:
    print(f"Sessions parsed: {len(sessions)}")
    print()

    for s in sessions:
        r = s.result
        print(f"{'=' * 60}")
        source_hint = f" source={s.data_source}" if s.agent_type == "opencode" else ""
        print(f"Session [{s.phase}] ({s.agent_type}{source_hint})  id={s.session_id[:8]}...")
        print(f"{'=' * 60}")

        if s.init:
            print(f"  Model:          {s.init.model}")
            print(f"  CC version:     {s.init.claude_code_version}")
            print(f"  Tools available:{len(s.init.tool_names)}")

        main_turns = len(s.conversation)
        main_with_thinking = sum(1 for t in s.conversation if t.thinking_texts)
        main_tool_uses = count_tools_in_conversation(s.conversation)
        agent_calls = [
            tu for t in s.conversation for tu in t.tool_uses
            if tu.name.lower() in ("agent", "task") and tu.subagent is not None
        ]

        print(f"\n  --- Main conversation ---")
        print(f"  Turns (main agent API calls): {main_turns}")
        print(f"  Turns with extended thinking: {main_with_thinking}")
        print(f"  Tool calls by name:           {dict(sorted(main_tool_uses.items()))}")
        print(f"  Agent (sub-agent) calls:      {len(agent_calls)}")

        frozen_tasks = [
            tu for t in s.conversation for tu in t.tool_uses if tu.frozen
        ]
        if frozen_tasks:
            print(f"  ⚠ FROZEN tool calls:          {len(frozen_tasks)} "
                  "(still running when the process ended)")

        stall_ms = _stall_gap_ms(s)
        if stall_ms:
            print(
                f"  ⚠ STALL: no session activity for {_format_duration(stall_ms)} "
                f"before the process ended (process ran "
                f"{_format_duration(s.process_wall_ms)}, session active "
                f"{_format_duration(s.wall_clock_ms)}) — likely stalled until "
                "the harness timeout killed it"
            )

        for ag in agent_calls:
            st = ag.subagent
            if st:
                sub_turns = len(st.conversation)
                sub_tools = count_tools_in_conversation(st.conversation)
                prog_steps = len(st.progress_snapshots)
                unfinished = ag.frozen or st.status in ("running", "pending")
                if unfinished and _session_ended(s):
                    frozen_note = "  ⚠ FROZEN (never completed; process ended mid-task)"
                elif unfinished:
                    frozen_note = "  ⏳ in progress (live trace — session still running)"
                else:
                    frozen_note = ""
                print(f"\n  --- Sub-agent [{st.description}] ---")
                print(f"    task_id:        {st.task_id}")
                print(f"    status:         {st.status}{frozen_note}")
                print(f"    turns:          {sub_turns}")
                print(f"    tool calls:     {dict(sorted(sub_tools.items()))}")
                print(f"    progress snaps: {prog_steps}")
                print(f"    total_tokens:   {st.total_tokens}")
                print(f"    total_tool_uses:{st.total_tool_uses}")
                print(f"    duration_ms:    {st.duration_ms}")

        monitoring_subtypes: dict[str, int] = defaultdict(int)
        for m in s.monitoring:
            key = f"{m.type}/{m.subtype}" if m.subtype else m.type
            monitoring_subtypes[key] += 1
        print(f"\n  --- Monitoring events ---")
        for k, v in sorted(monitoring_subtypes.items()):
            print(f"    {k}: {v}")

        if r:
            print(f"\n  --- Result ---")
            print(f"    stop_reason:    {r.stop_reason}")
            print(f"    is_error:       {r.is_error}")
            print(f"    num_turns:      {r.num_turns}")
            print(f"    duration_ms:    {r.duration_ms}")
            print(f"    total_cost_usd: ${r.total_cost_usd:.4f}")
            for model, mu in r.model_usage.items():
                print(f"    [{model}]")
                print(f"      input_tokens:  {mu.input_tokens}")
                if mu.reasoning_tokens:
                    print(
                        f"      output_tokens: {mu.output_tokens} "
                        f"(incl. {mu.reasoning_tokens} reasoning)"
                    )
                else:
                    print(f"      output_tokens: {mu.output_tokens}")
                print(f"      cache_read:    {mu.cache_read_tokens}")
                print(f"      cache_create:  {mu.cache_creation_tokens}")
                print(f"      cost_usd:      ${mu.cost_usd:.4f}")
        print()


# ---------------------------------------------------------------------------
# Human-readable history printer
# ---------------------------------------------------------------------------

def _truncate(text: str, max_len: int = 30000) -> str:
    text = text.strip()
    if len(text) <= max_len:
        return text
    return text[:max_len] + f"  …[+{len(text) - max_len} chars]"


def _apply_patch_files(patch_text: str) -> list[str]:
    """
    Extract per-file operations from an OpenAI `apply_patch` envelope
    (`*** Add File: p`, `*** Update File: p`, `*** Delete File: p`).
    GPT-family models on OpenCode write files through this tool instead of
    write/edit, so the visualizer must treat it as a write action.
    """
    ops = []
    for line in patch_text.splitlines():
        line = line.strip()
        for marker, tag in (("*** Add File:", "add"),
                            ("*** Update File:", "update"),
                            ("*** Delete File:", "delete")):
            if line.startswith(marker):
                ops.append(f"{tag} {line[len(marker):].strip()}")
    return ops


def _fmt_tool_input(name: str, inp: dict) -> str:
    """Format tool input as a compact one-or-few-liner."""
    name_lower = name.lower()
    if not isinstance(inp, dict):
        return str(inp)[:200]
    if name_lower == "apply_patch":
        patch = inp.get("patchText", inp.get("patch", "")) or ""
        ops = _apply_patch_files(patch)
        files = "; ".join(ops) if ops else "(no file headers in patch)"
        return f"{files}  ({len(patch)} chars)"
    if name_lower == "bash":
        cmd = inp.get("command", inp.get("input", "")).strip().replace("\n", " ↵ ")
        desc = inp.get("description", "")
        if desc:
            return f"[{desc}]\n         $ {_truncate(cmd)}"
        return f"$ {_truncate(cmd)}"
    if name_lower in ("read", "write", "edit"):
        path = inp.get("file_path", inp.get("filePath", inp.get("path", "")))
        extra = ""
        if name_lower == "write":
            content = inp.get("content", "")
            extra = f"  ({len(content)} chars)"
        elif name_lower == "edit":
            old = inp.get("old_string", inp.get("oldString", ""))[:60].replace("\n", "↵")
            new = inp.get("new_string", inp.get("newString", ""))[:60].replace("\n", "↵")
            extra = f"\n         - {old!r}\n         + {new!r}"
        return f"{path}{extra}"
    if name_lower in ("agent", "task"):
        desc = inp.get("description", "")
        sub_type = inp.get("subagent_type", "")
        prompt_preview = _truncate(inp.get("prompt", ""))
        return f"[{sub_type}] {desc}\n         prompt: {prompt_preview}"
    # Generic fallback
    parts = []
    for k, v in inp.items():
        v_str = str(v).replace("\n", "↵")
        parts.append(f"{k}={_truncate(v_str)}")
    return "  ".join(parts)


def _fmt_turns(turns: list[Turn], out: list[str], indent: str = "") -> None:
    """Append formatted turn lines into `out` (a list[str] for efficient joining)."""
    for turn in turns:
        out.append(f"{indent}┌─ Turn {turn.turn_index} {'─' * max(0, 60 - len(indent) - 10)}")

        # Thinking blocks
        for thinking in turn.thinking_texts:
            preview = _truncate(thinking)
            out.append(f"{indent}│  💭 Thinking: ({len(thinking)} chars)")
            for line in preview.splitlines():
                out.append(f"{indent}│     {line}")

        # Text blocks
        for text in turn.texts:
            out.append(f"{indent}│  💬 {_truncate(text)}")

        # Tool calls
        for tu in turn.tool_uses:
            is_agent = tu.name.lower() in ("agent", "task")
            icon = "🤖" if is_agent else "🔧"
            fmt_input = _fmt_tool_input(tu.name, tu.input)
            input_lines = fmt_input.splitlines()
            # Interrupted/aborted tool calls (a killed session leaves
            # `tool:"unknown"` stubs with `input:{}`) format to an empty
            # string, so `input_lines` can be empty — guard the head access.
            head = input_lines[0] if input_lines else ""
            out.append(f"{indent}│  {icon} Tool: {tu.name}  {head}")
            for extra_line in input_lines[1:]:
                out.append(f"{indent}│     {extra_line}")

            # Sub-agent: recurse with deeper indentation
            if is_agent and tu.subagent:
                sa = tu.subagent
                out.append(f"{indent}│  ╔═ Sub-agent [{sa.description}] ({'─'*20})")
                out.append(f"{indent}│  ║  task_id: {sa.task_id}  status: {sa.status}")
                _fmt_turns(sa.conversation, out, indent=indent + "│  ║  ")
                out.append(
                    f"{indent}│  ╚═ Sub-agent done  "
                    f"tokens={sa.total_tokens}  tools={sa.total_tool_uses}  "
                    f"duration={sa.duration_ms}ms"
                )

            # Tool result (after sub-agent block when applicable)
            if tu.result:
                result_text = _truncate(tu.result.content)
                err_tag = " [ERROR]" if tu.result.is_error else ""
                out.append(f"{indent}│  📥 Result{err_tag}: ({len(tu.result.content)} chars)")
                for line in result_text.splitlines():
                    out.append(f"{indent}│     {line}")

        out.append(f"{indent}└{'─' * max(0, 62 - len(indent))}")
        out.append("")



# ---------------------------------------------------------------------------
# File I/O Analysis (--file-io)
# ---------------------------------------------------------------------------

def _normalize_io_path(path: str) -> str:
    """Normalize /tmp/.tmpXXX/translated_rust/... to relative form."""
    path = re.sub(r'^/tmp/\.tmp\w+/translated_rust/', '', path)
    path = re.sub(r'^\./', '', path)
    return path.strip()


def _extract_bash_files(cmd: str) -> list[str]:
    """Extract file paths referenced in a Bash command."""
    files = set()
    for m in re.finditer(r'(?:c_src|src)/[\w./-]+\.(?:c|h|rs|toml)', cmd):
        files.add(m.group(0))
    for m in re.finditer(r'/tmp/\.tmp\w+/translated_rust/([\w./-]+\.(?:c|h|rs|toml))', cmd):
        files.add(m.group(1))
    return list(files)


def _estimate_read_bytes(offset: int, limit: int) -> int:
    return max(0, (limit - offset + 1)) * 80


def _flatten_tool_uses(sessions: list[Session]) -> list[tuple[str, ToolUse]]:
    """Yield (session_id, ToolUse) recursively including sub-agents."""
    stack: list[tuple[str, Session]] = [(s.session_id, s) for s in sessions]
    while stack:
        sid, sess = stack.pop()
        for turn in sess.conversation:
            for tu in turn.tool_uses:
                yield (sid, tu)
                if tu.subagent is not None:
                    stack.append((tu.subagent.task_id or sid, tu.subagent))


def generate_file_io_report(sessions: list[Session]) -> dict:
    """Return structured dict of per-file I/O patterns."""
    file_ops: dict[str, list[dict]] = defaultdict(list)

    for sid, tu in _flatten_tool_uses(sessions):
        inp = tu.input if isinstance(tu.input, dict) else {}
        ts = ''
        if tu.result and hasattr(tu.result, 'timestamp'):
            ts = tu.result.timestamp or ''

        name_lower = tu.name.lower()
        if name_lower == 'read':
            fp = inp.get('file_path', inp.get('filePath', ''))
            if not fp:
                continue
            np = _normalize_io_path(fp)
            offset = inp.get('offset', 1)
            limit = inp.get('limit', 500)
            if isinstance(offset, list): offset = offset[0] if offset else 1
            if isinstance(limit, list): limit = limit[0] if limit else 500
            if not isinstance(offset, int): offset = 1
            if not isinstance(limit, int): limit = 500
            file_ops[np].append({
                'op': 'read', 'session': sid, 'ts': ts,
                'bytes': _estimate_read_bytes(offset, limit),
                'offset': offset, 'limit': limit,
            })

        elif name_lower == 'write':
            fp = inp.get('file_path', inp.get('filePath', ''))
            if not fp:
                continue
            np = _normalize_io_path(fp)
            clen = len(inp.get('content', '')) if isinstance(inp.get('content', ''), str) else 0
            file_ops[np].append({
                'op': 'write', 'session': sid, 'ts': ts, 'bytes': clen,
            })

        elif name_lower == 'edit':
            fp = inp.get('file_path', inp.get('filePath', ''))
            if not fp:
                continue
            np = _normalize_io_path(fp)
            old_l = len(inp.get('old_string', inp.get('oldString', ''))) if isinstance(inp.get('old_string', inp.get('oldString', '')), str) else 0
            new_l = len(inp.get('new_string', inp.get('newString', ''))) if isinstance(inp.get('new_string', inp.get('newString', '')), str) else 0
            file_ops[np].append({
                'op': 'edit', 'session': sid, 'ts': ts,
                'old_len': old_l, 'new_len': new_l,
            })

        elif name_lower == 'bash':
            cmd = inp.get('command', '')
            for fp in _extract_bash_files(cmd):
                np = _normalize_io_path(fp)
                file_ops[np].append({
                    'op': 'bash_ref', 'session': sid, 'ts': ts,
                    'cmd': cmd[:200],
                })

    files = {}
    for fp in sorted(file_ops.keys()):
        ops = file_ops[fp]
        reads = [o for o in ops if o['op'] == 'read']
        writes = [o for o in ops if o['op'] == 'write']
        edits = [o for o in ops if o['op'] == 'edit']
        bash_refs = [o for o in ops if o['op'] == 'bash_ref']
        files[fp] = {
            'read_count': len(reads),
            'write_count': len(writes),
            'edit_count': len(edits),
            'bash_ref_count': len(bash_refs),
            'total_read_bytes': sum(o.get('bytes', 0) for o in reads),
            'total_write_bytes': sum(o.get('bytes', 0) for o in writes),
            'sessions': sorted(set(o['session'][:8] for o in ops)),
            'ops': ops,
        }

    return {
        'files': files,
        'summary': {
            'total_files': len(files),
            'total_read_bytes': sum(f['total_read_bytes'] for f in files.values()),
            'total_write_bytes': sum(f['total_write_bytes'] for f in files.values()),
            'total_reads': sum(f['read_count'] for f in files.values()),
            'total_writes': sum(f['write_count'] for f in files.values()),
            'total_edits': sum(f['edit_count'] for f in files.values()),
            'total_bash_refs': sum(f['bash_ref_count'] for f in files.values()),
        },
    }


def build_readable_history(sessions: list[Session]) -> str:
    """
    Build a human-readable narrative of the full agent execution.
    Returns a single string. Uses list accumulation + join for efficiency
    since the output can be very large (multiple MB).
    """
    out: list[str] = []
    total = len(sessions)

    for idx, s in enumerate(sessions, 1):
        r = s.result
        duration_s = f"{r.duration_ms / 1000:.1f}s" if r else "?"
        cost = f"${r.total_cost_usd:.4f}" if r else "?"

        out.append("")
        out.append("=" * 70)
        out.append(f"  Session {idx}/{total} — {s.phase.upper()}")
        out.append(
            f"  model: {s.init.model if s.init else '?'}  "
            f"duration: {duration_s}  cost: {cost}"
        )
        out.append("=" * 70)
        out.append("")

        # Surface rate-limit events
        for m in s.monitoring:
            if m.type == "rate_limit_event" or m.subtype == "rate_limit_event":
                info = m.raw.get("rate_limit_info", {})
                out.append(
                    f"  ⚠️  Rate limit: status={info.get('status')}  "
                    f"type={info.get('rateLimitType')}"
                )

        out.append("")
        _fmt_turns(s.conversation, out)

        if r:
            out.append(
                f"  ✅ Session ended: {r.stop_reason}  "
                f"turns={r.num_turns}  cost={cost}"
            )
            for model, mu in r.model_usage.items():
                short = model.split("-")[1] if "-" in model else model
                out.append(
                    f"     [{short}] in={mu.input_tokens} out={mu.output_tokens}  "
                    f"cache_read={mu.cache_read_tokens}  ${mu.cost_usd:.4f}"
                )
        out.append("")

    return "\n".join(out)


# ---------------------------------------------------------------------------
# Timeline Visualization (SVG)
#
# Each turn becomes one horizontal bar. The bar is split into colored
# segments, one per operation, in execution order. Segment width is
# proportional to the character count that flowed through that operation:
#
#   read  — content read INTO context (tool_result of Read/Glob/Grep,
#           or Bash commands like cat/ls/grep that pull data in)
#   think — model output (thinking blocks + assistant text)
#   write — content the agent produced (Write content, Edit new_string,
#           Bash redirects/sed -i)
#   build — execution-style operations (cargo/cmake/make/gcc/python/...)
#           sized by tool_result length (the output that re-entered context)
#   other — anything we couldn't classify
#
# Bar TOTAL width is normalized to the longest turn in the trace, so a
# row that fills the whole strip = the heaviest turn; short rows = short
# turns. Hover any segment to see the full command/preview as a tooltip.
# ---------------------------------------------------------------------------

CAT_READ, CAT_THINK, CAT_WRITE, CAT_BUILD, CAT_SUBAGENT, CAT_OTHER = (
    "read", "think", "write", "build", "subagent", "other",
)

CAT_COLORS = {
    CAT_READ:     "#85c955",   # green
    CAT_THINK:    "#5b9bd5",   # blue
    CAT_WRITE:    "#e8a020",   # amber
    CAT_BUILD:    "#e05050",   # red
    CAT_SUBAGENT: "#9b80c8",   # purple-grey
    CAT_OTHER:    "#555555",   # dark grey
}

# Dark theme palette
_BG          = "#1e1e1e"
_ROW_ALT     = "#252525"
_TEXT        = "#cccccc"
_TEXT_DIM    = "#888888"
_GRID        = "#333333"
_TITLE       = "#e0e0e0"

# Bash command classification by leading word.
_READ_CMDS = {
    # Real shell commands
    "ls", "cat", "head", "tail", "find", "grep", "rg", "wc", "file",
    "stat", "du", "pwd", "which", "tree", "diff", "less", "more",
    "nm", "objdump", "readelf", "strings", "od", "xxd", "awk", "sed",
    "jq", "column", "sort", "uniq", "cut", "tr",
    "ps", "pgrep", "pidof", "ldd", "ldconfig",
    "echo", "printf",
    # English-verb intent (used by async sub-agent progress descriptions
    # that carry a prose summary instead of an actual shell command).
    "list", "show", "view", "check", "verify", "search", "count",
    "look", "examine", "read", "inspect", "scan", "lookup",
}
_WRITE_CMDS = {
    "cp", "mv", "mkdir", "touch", "rm", "rmdir", "ln", "chmod", "chown",
    "patch", "tar", "unzip", "zip",
    # English-verb intent
    "create", "delete", "remove", "write", "save", "make_dir", "rename",
}
_BUILD_CMDS = {
    "cargo", "cmake", "make", "ninja", "gcc", "clang", "g++", "clang++",
    "rustc", "rustfmt", "clippy", "go", "python", "python3", "node",
    "npm", "yarn", "pnpm",
    "ctest", "bear", "opt", "llvm-link", "llc", "ld", "ar",
    "bash", "sh", "zsh", "fish",
    # English-verb intent
    "compile", "build", "test", "run",
}


def _peel_bash(cmd: str) -> str:
    """Strip wrapper prefixes / loop scaffolding and return a simple head command.

    Handles: set -..., timeout, cd X (&&|newline), env, leading comments,
    backslash line-continuation, bare VAR=value assignments, echo "..." &&,
    and for/while/until ...; do BODY; done (returns BODY's head)."""
    cmd = cmd.strip()
    while True:
        new_cmd = cmd
        # Leading comment lines: "# foo\n..."
        new_cmd = re.sub(r"^#[^\n]*\n+", "", new_cmd)
        # Backslash line-continuation at start: "\\\n cmd"
        new_cmd = re.sub(r"^\\\s*\n+", "", new_cmd)
        # set -e; / set -o pipefail; / etc
        new_cmd = re.sub(r"^set\s+-\S+(\s+\S+)*\s*;\s*", "", new_cmd)
        # timeout NN
        new_cmd = re.sub(r"^timeout\s+\S+\s+", "", new_cmd)
        # cd X (followed by &&, ;, or newline)
        new_cmd = re.sub(r"^cd\s+\S+\s*(?:&&|;|\n)\s*", "", new_cmd)
        # env VAR=val ...
        new_cmd = re.sub(r"^env\s+(?:\w+=\S+\s+)+", "", new_cmd)
        # Bare VAR=val assignments (one or more), incl. VAR=$(...) and VAR="..."
        # Stops at the first token that isn't an assignment. Uses [\s\S] inside
        # $(...) and quoted strings so multiline values are handled. Trailing
        # separator can be whitespace or `;`.
        new_cmd = re.sub(
            r"""^(?:\w+=(?:"[^"]*"|'[^']*'|\$\([\s\S]*?\)|[^\s$"'(]\S*)[;\s]+)+""",
            "", new_cmd,
        )
        # echo "..." &&  (a "report header" before the real cmd)
        new_cmd = re.sub(
            r"""^echo\s+(?:"[^"]*"|'[^']*'|\S+)\s*&&\s*""",
            "", new_cmd,
        )
        new_cmd = new_cmd.lstrip()
        if new_cmd == cmd:
            break
        cmd = new_cmd

    # for/while/until ... do BODY; done — recurse into BODY
    m = re.match(
        r"^(?:for|while|until)\b.*?\bdo\s+(.+?)(?:;\s*done\b.*)?$",
        cmd, re.DOTALL,
    )
    if m:
        body = m.group(1).strip()
        if body and body != cmd:
            return _peel_bash(body)

    # if COND; then BODY; (else ...;)? fi — recurse into BODY
    m = re.match(
        r"^if\b.*?;\s*then\s+(.+?)(?:;\s*(?:elif|else|fi)\b.*)?$",
        cmd, re.DOTALL,
    )
    if m:
        body = m.group(1).strip()
        if body and body != cmd:
            return _peel_bash(body)

    # Split on first command separator (newline, |, ;, &&, ||).
    parts = re.split(r"[\|;\n]|&&|\|\|", cmd, maxsplit=1)
    return parts[0].strip()


def _classify_bash(cmd: str) -> str:
    """Classify a bash command by its primary purpose.

    Also handles English-prose descriptions emitted by async sub-agent
    progress events (e.g. "Check compilation", "List source files") by
    matching a lowercased leading verb against the same category sets."""
    # Write detection: redirect, tee, sed -i anywhere in the line.
    if (re.search(r"(^|\s)>>?\s", cmd)
            or re.search(r"\btee\s", cmd)
            or re.search(r"\bsed\s+-i\b", cmd)):
        return CAT_WRITE

    head = _peel_bash(cmd)
    if not head:
        return CAT_OTHER
    tokens = head.split()
    first = tokens[0] if tokens else ""
    first_lc = first.lower()

    if first.startswith(("./", "/", "../")):
        return CAT_BUILD
    # Lowercase comparison so English-verb prose ("Check ...", "List ...")
    # still matches. Real shell commands are already lowercase, so this
    # doesn't change their classification.
    if first_lc in _READ_CMDS:
        return CAT_READ
    if first_lc in _WRITE_CMDS:
        return CAT_WRITE
    if first_lc in _BUILD_CMDS:
        return CAT_BUILD
    return CAT_OTHER


def _classify_tool(tu: ToolUse) -> str:
    name_lower = tu.name.lower()
    if name_lower in ("read", "glob", "grep"):
        return CAT_READ
    if name_lower in ("write", "edit", "notebookedit", "apply_patch"):
        return CAT_WRITE
    if name_lower == "bash":
        cmd = tu.input.get("command", "") if isinstance(tu.input, dict) else ""
        return _classify_bash(cmd)
    if name_lower in ("agent", "task", "workflow"):
        return CAT_SUBAGENT
    if name_lower in ("todowrite", "lsp"):
        return CAT_OTHER
    return CAT_OTHER


def _tool_size(tu: ToolUse) -> int:
    """Visualization weight for a tool use, in characters."""
    if tu.size_override is not None:
        return tu.size_override
    name_lower = tu.name.lower()
    if name_lower == "write":
        return len(tu.input.get("content", "")) if isinstance(tu.input, dict) else 0
    if name_lower == "edit":
        return len(tu.input.get("new_string", "")) if isinstance(tu.input, dict) else 0
    if name_lower == "apply_patch":
        # Weight by the patch body; a few events carry an empty patchText
        # (observed for PLAN.md updates), fall back to the result size.
        patch = tu.input.get("patchText", "") if isinstance(tu.input, dict) else ""
        if patch:
            return len(patch)
    # All others: characters that came back into context.
    return len(tu.result.content) if tu.result else 0


# Map from progress-event description prefix (per `last_tool_name`) to
# regex that strips the prefix to recover the closest thing to the original
# tool input. The prefix scheme is set by the framework, see how each tool
# emits task_progress descriptions: e.g. Read → "Reading <path>", Bash →
# "Running <agent-supplied-description>", etc.
_PROGRESS_PREFIX = {
    "Read":  re.compile(r"^Reading\s+", re.IGNORECASE),
    "Write": re.compile(r"^Writing\s+", re.IGNORECASE),
    "Edit":  re.compile(r"^Editing\s+", re.IGNORECASE),
    "Glob":  re.compile(r"^Finding\s+", re.IGNORECASE),
    "Grep":  re.compile(r"^Searching for\s+", re.IGNORECASE),
    "Bash":  re.compile(r"^Running\s+", re.IGNORECASE),
}


def _synthesize_async_subagent_turns(
    snapshots: list[ProgressSnapshot],
) -> list[Turn]:
    """Async sub-agents have no parented conversation records in the trace,
    only `task_progress` snapshots. Build one pseudo-Turn per snapshot, each
    holding a single ToolUse named after `last_tool_name`. Sizes use
    `total_tokens` deltas as a proxy for "context grown by this tool call".

    The synthesized turns are approximate — exact tool inputs/outputs are not
    in the trace — but they are enough for the visualizer to render the
    activity, and tooltips carry the framework-supplied description.

    Per-tool input population: framework progress descriptions follow stable
    prefix patterns ("Reading <path>", "Running <bash-desc>", ...). We strip
    the prefix and put the body into the field _classify_tool / _segment_turn
    expects (command for Bash, file_path for Read/Write/Edit, pattern for
    Glob/Grep), so downstream classifiers see something useful instead of an
    empty input dict.
    """
    turns: list[Turn] = []
    prev_tokens = 0
    for idx, snap in enumerate(snapshots):
        token_delta = max(snap.total_tokens - prev_tokens, 0)
        prev_tokens = snap.total_tokens
        tool_name = snap.last_tool_name or "Bash"
        # Strip the framework prefix to recover the body.
        body = snap.description or ""
        prefix_re = _PROGRESS_PREFIX.get(tool_name)
        if prefix_re is not None:
            body = prefix_re.sub("", body, count=1)
        # Populate the input field downstream code looks at, by tool kind.
        synthetic_input: dict = {"description": snap.description}
        if tool_name == "Bash":
            synthetic_input["command"] = body
        elif tool_name in ("Read", "Write", "Edit"):
            synthetic_input["file_path"] = body
        elif tool_name in ("Glob", "Grep"):
            synthetic_input["pattern"] = body
        tu = ToolUse(
            id=f"async-progress-{idx}",
            name=tool_name,
            input=synthetic_input,
            size_override=token_delta,
        )
        turn = Turn(turn_index=len(turns) + 1)
        turn.content_blocks.append(ContentBlock(type="tool_use", tool_use=tu))
        turns.append(turn)
    return turns


def _synthesize_workflow_agent_turns(
    progress_events: list[dict],
    prev_tokens: int = 0,
) -> tuple[list[Turn], int]:
    """Workflow tasks contain multiple sequential agents, each tracked by
    `task_progress` events.  Build one pseudo-Turn per progress event,
    using token deltas between consecutive events to approximate work done.

    The `workflow_progress` array in each event contains `workflow_agent`
    entries with `lastToolName` and `lastToolSummary`, which describe the
    actual tool the agent was using.  Use these for colorized tool names
    instead of the generic "Other" fallback.

    `prev_tokens` carries across agents because total_tokens is cumulative
    across the entire workflow.  Returns (turns, final_prev_tokens).
    """
    turns: list[Turn] = []
    for idx, ev in enumerate(progress_events):
        usage = ev.get("usage", {})
        total_tokens = usage.get("total_tokens", 0)
        token_delta = max(total_tokens - prev_tokens, 0)
        prev_tokens = total_tokens

        desc = ev.get("description", "") or "workflow-step"

        # Extract phase title and actual tool info from workflow_progress.
        phase_title = ""
        tool_name = ""
        tool_summary = ""
        for wp in ev.get("workflow_progress") or []:
            if wp.get("type") == "workflow_phase":
                phase_title = wp.get("title", "")
            elif wp.get("type") == "workflow_agent":
                # Prefer the most recent agent entry's tool info.
                tn = wp.get("lastToolName", "")
                ts = wp.get("lastToolSummary", "")
                if tn:
                    tool_name = tn
                if ts:
                    tool_summary = ts

        label = f"{phase_title}: {desc}" if phase_title and phase_title not in desc else desc

        # Build a ToolUse with the real tool name so _classify_tool assigns
        # the correct color (Read=blue, Write=green, Bash=build/yellow, etc.).
        effective_name = tool_name if tool_name else "Other"
        synthetic_input: dict = {"description": label}
        if tool_name:
            if tool_name.lower() == "bash":
                synthetic_input["command"] = tool_summary
            elif tool_name.lower() in ("read", "write", "edit"):
                synthetic_input["file_path"] = tool_summary
            elif tool_name.lower() == "apply_patch":
                synthetic_input["patchText"] = tool_summary
            elif tool_name.lower() in ("glob", "grep"):
                synthetic_input["pattern"] = tool_summary

        tu = ToolUse(
            id=f"workflow-step-{idx}",
            name=effective_name,
            input=synthetic_input,
            size_override=token_delta,
        )
        turn = Turn(turn_index=len(turns) + 1)
        turn.content_blocks.append(ContentBlock(type="tool_use", tool_use=tu))
        turns.append(turn)
    return turns, prev_tokens


def _think_size(turn: Turn) -> int:
    return sum(
        len(b.thinking or "") if b.type == "thinking" else len(b.text or "")
        for b in turn.content_blocks
        if b.type in ("thinking", "text")
    )


@dataclass
class _Segment:
    category: str
    size: int
    tooltip: str

PREVIEW_SIZE = 400  # chars of tool input/result to show in tooltip

def _segment_turn(turn: Turn) -> list[_Segment]:
    """Break a turn into ordered segments; think first, then tool ops."""
    segs: list[_Segment] = []

    think_size = _think_size(turn)
    if think_size > 0:
        previews: list[str] = []
        for b in turn.content_blocks:
            if b.type == "thinking" and b.thinking:
                previews.append("💭 " + b.thinking[:200].replace("\n", " "))
            elif b.type == "text" and b.text:
                previews.append("💬 " + b.text[:200].replace("\n", " "))
        tip = "\n".join(previews[:4]) if previews else f"think ({think_size})"
        segs.append(_Segment(CAT_THINK, think_size, tip))

    for tu in turn.tool_uses:
        size = _tool_size(tu)
        if size <= 0:
            continue
        cat = _classify_tool(tu)
        # For synthesized async-sub-agent ToolUses the only input we have is
        # `description` (lifted from a task_progress snapshot). Use it as a
        # universal fallback when the usual fields aren't present.
        desc_fallback = tu.input.get("description", "") if isinstance(tu.input, dict) else ""
        name_lower = tu.name.lower()
        if name_lower == "bash":
            cmd = tu.input.get("command", "") if isinstance(tu.input, dict) else ""
            tip = f"$ {cmd[:400]}" if cmd else f"Bash: {desc_fallback}"
        elif name_lower in ("read", "glob", "grep"):
            target = (
                (tu.input.get("file_path") if isinstance(tu.input, dict) else None)
                or (tu.input.get("filePath") if isinstance(tu.input, dict) else None)
                or (tu.input.get("pattern", "") if isinstance(tu.input, dict) else "")
                or desc_fallback
            )
            tip = f"{tu.name}: {target}"
        elif name_lower in ("write", "edit"):
            target = ((tu.input.get("file_path") or tu.input.get("filePath") or "") if isinstance(tu.input, dict) else "") or desc_fallback
            tip = f"{tu.name}: {target}"
        elif name_lower == "apply_patch":
            patch = tu.input.get("patchText", "") if isinstance(tu.input, dict) else ""
            ops = _apply_patch_files(patch)
            target = "; ".join(ops) or desc_fallback
            tip = f"{tu.name}: {target}"
        elif name_lower in ("agent", "task", "workflow"):
            desc = ""
            prompt_text = ""
            result_preview = ""
            if isinstance(tu.input, dict):
                desc = tu.input.get("description", "")
                prompt_text = tu.input.get("prompt", "")
            # Workflow ToolUse.input has only "script"; fall back to SubAgent.
            if tu.subagent:
                desc = desc or tu.subagent.description or ""
                prompt_text = prompt_text or tu.subagent.prompt or ""
            prompt_preview = prompt_text[:PREVIEW_SIZE].replace("\n", " ") if prompt_text else ""
            result_preview = (tu.result.content[:PREVIEW_SIZE] if tu.result else "").replace("\n", " ")
            frozen_note = "⚠ FROZEN — never completed\n" if tu.frozen else ""
            tip = f"{frozen_note}[task] {desc}\n[prompt] {prompt_preview}\n[result] {result_preview}"
        else:
            tip = f"{tu.name}: {desc_fallback}" if desc_fallback else tu.name
        segs.append(_Segment(cat, size, tip))

    return segs


@dataclass
class _Row:
    """One line in the SVG timeline. Either a turn (segments), a context
    compaction divider (compact set), a blank spacer (spacer=True), or a
    session summary banner (summary_text set).
    `height` overrides the default row height."""
    label: str
    depth: int
    segments: list[_Segment] = field(default_factory=list)
    compact: Optional[CompactEvent] = None
    spacer: bool = False
    height: Optional[int] = None
    summary_text: Optional[str] = None


@dataclass
class _Group:
    """A contiguous span of rows belonging to one sub-agent dispatch.
    Rendered as a thin vertical guide line in the indent gutter so siblings
    are visually grouped. Async sub-agents draw the line dashed because the
    row order is approximate (synthesized from progress snapshots)."""
    depth: int       # depth of the sub-agent's own rows (parent_depth + 1)
    start_idx: int   # first row index belonging to this sub-agent (inclusive)
    end_idx: int     # last row index (inclusive)
    tool_use: Optional[ToolUse] = None  # the parent's Agent ToolUse that spawned this group
    is_async: bool = False
    session_ended: bool = True  # False for a live trace (no result event yet)


def _session_ended(s: Session) -> bool:
    """
    Whether the process behind this session is known to have finished.

    A Claude stream trace ends with a `result` event; without one the trace
    is either still being written (live visualization of a running session)
    or was killed hard. A sub-agent with status "running" must NOT be
    reported as frozen unless the session actually ended — otherwise every
    in-flight sync sub-agent in a live trace shows up as a false FROZEN.

    OpenCode sessions can also be live: live-exports (and JSONL tails) of a
    running session report in-flight tools as status "running" exactly like
    a genuinely frozen one. The reliable end signal is the agent_runner
    phase window: `process_wall_ms` is only set once the trace contains the
    closing "Exporting"/"Appended" runner marker, which a live run has not
    written yet. (Session.result is synthesized for OpenCode and says
    nothing about liveness.)
    """
    if s.agent_type == "claude":
        return s.result is not None
    return bool(s.process_wall_ms)


def _flatten_rows(sessions: list[Session]) -> tuple[list[_Row], list[_Group]]:
    """
    Flatten sessions + sub-agents into renderable rows.

    Sub-agent turns are emitted immediately after the parent turn that
    spawned them, with depth = parent_depth + 1, so the SVG renderer can
    indent them visually. Top-level `compact_boundary` events are inserted
    between the turns they fall between.

    Also returns a list of `_Group` markers, one per sub-agent dispatch,
    so the renderer can draw a vertical guide line spanning each group.
    """
    rows: list[_Row] = []
    groups: list[_Group] = []

    def emit(turns: list[Turn], prefix: str, depth: int,
             compacts_by_after: dict[int, list[CompactEvent]],
             session_ended: bool) -> None:
        # Compaction can occur before any turn was issued (rare).
        for ev in compacts_by_after.get(0, []):
            rows.append(_Row(label="", depth=depth, compact=ev))

        for t in turns:
            label = f"{prefix}T{t.turn_index}"
            rows.append(_Row(label=label, depth=depth, segments=_segment_turn(t)))
            # Number sub-agents within this turn so labels disambiguate them.
            # A turn can issue several Agent calls (true parallel via multiple
            # tool_use blocks in one message); without numbering, every
            # sub-agent's "T1" looks identical to every other sub-agent's "T1".
            sub_idx = 0
            sub_count = sum(
                1 for tu in t.tool_uses
                if tu.name.lower() in ("agent", "task", "workflow") and tu.subagent is not None
            )
            for tu in t.tool_uses:
                if tu.name.lower() in ("agent", "task", "workflow") and tu.subagent:
                    sub_idx += 1
                    sub_prefix = f"{prefix}T{t.turn_index}/A{sub_idx}:"
                    grp_start = len(rows)
                    sub_compacts: dict[int, list[CompactEvent]] = defaultdict(list)
                    for ev in tu.subagent.compact_events:
                        sub_compacts[ev.after_turn_index].append(ev)
                    emit(tu.subagent.conversation, sub_prefix, depth + 1,
                         sub_compacts, session_ended)
                    grp_end = len(rows) - 1
                    if grp_end >= grp_start:
                        groups.append(_Group(
                            depth=depth + 1,
                            start_idx=grp_start, end_idx=grp_end,
                            tool_use=tu,
                            is_async=tu.subagent.is_async,
                            session_ended=session_ended,
                        ))
                    # Insert a thin spacer between sibling sub-agents so the
                    # boundary between A{n} and A{n+1} is visually obvious
                    # without wasting a full row of vertical space.
                    if sub_idx < sub_count:
                        rows.append(_Row(
                            label="", depth=depth + 1, spacer=True, height=8,
                        ))
            for ev in compacts_by_after.get(t.turn_index, []):
                rows.append(_Row(label="", depth=depth, compact=ev))

    for s_idx, s in enumerate(sessions, 1):
        # Full-height blank row between sessions, so the visual transition
        # from translation → verification is unmistakable.
        if s_idx > 1:
            rows.append(_Row(label="", depth=0, spacer=True))
        prefix = f"S{s_idx}." if len(sessions) > 1 else ""
        # Bucket compacts by the main turn they follow.
        by_after: dict[int, list[CompactEvent]] = defaultdict(list)
        for ev in s.compact_events:
            by_after[ev.after_turn_index].append(ev)
        emit(s.conversation, prefix, 0, by_after, _session_ended(s))
        # Append a summary banner at the end of this session.
        rows.append(_Row(
            label="", depth=0, summary_text=_format_session_summary(s, s_idx),
        ))

    return rows, groups


def _count_subagents(turns: list[Turn]) -> tuple[int, int]:
    """Count (sync, async) sub-agents recursively across the turn tree."""
    sync = 0
    async_ = 0
    for t in turns:
        for blk in t.content_blocks:
            if blk.type == "tool_use" and blk.tool_use and blk.tool_use.subagent:
                sa = blk.tool_use.subagent
                if sa.is_async:
                    async_ += 1
                else:
                    sync += 1
                ns, na = _count_subagents(sa.conversation)
                sync += ns
                async_ += na
    return sync, async_


def _format_duration(ms: int) -> str:
    if ms <= 0: return "?"
    s = ms // 1000
    if s < 60: return f"{s}s"
    m, s = divmod(s, 60)
    if m < 60: return f"{m}m{s:02d}s"
    h, m = divmod(m, 60)
    return f"{h}h{m:02d}m"


_STALL_GAP_MS = 600_000  # ≥10 min between last session activity and process death


def _stall_gap_ms(s: Session) -> int:
    """Dead time between the session's last recorded activity and the agent
    process's end. Nonzero only when agent_runner markers were found and the
    gap exceeds `_STALL_GAP_MS` — the signature of a stalled process that sat
    idle until the harness timeout killed it."""
    if (
        s.process_wall_ms
        and s.wall_clock_ms
        and s.process_wall_ms > s.wall_clock_ms + _STALL_GAP_MS
    ):
        return s.process_wall_ms - s.wall_clock_ms
    return 0


def _count_frozen_tools(turns: list[Turn]) -> int:
    """Recursively count tool calls frozen mid-flight (never completed)."""
    n = 0
    for t in turns:
        for tu in t.tool_uses:
            if tu.frozen:
                n += 1
            if tu.subagent is not None:
                n += _count_frozen_tools(tu.subagent.conversation)
    return n


def _format_compact_int(n: int) -> str:
    if n >= 1_000_000: return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:     return f"{n / 1_000:.1f}k"
    return str(n)


def _sum_subagent_tokens(turns: list[Turn]) -> dict[str, int]:
    """Recursively sum token usage from all sub-agents in a turn tree.

    Returns a dict with keys: input, output, cache_read, cache_write, total, unclassified.
    - input/output/cache_read/cache_write: tokens from sub-agents that have per-turn
      breakdown (OpenCode exports).
    - unclassified: flat total_tokens from sub-agents without breakdown (Claude).
    - total: sum of all tokens regardless of classification.
    """
    agg = {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "total": 0, "unclassified": 0}

    for turn in turns:
        for blk in turn.content_blocks:
            if blk.type != "tool_use" or not blk.tool_use or not blk.tool_use.subagent:
                continue
            sa = blk.tool_use.subagent

            # Prefer per-turn breakdown from the sub-agent's conversation.
            has_breakdown = False
            for t in sa.conversation:
                if t.usage:
                    agg["input"] += t.usage.input_tokens
                    agg["output"] += t.usage.output_tokens
                    agg["cache_read"] += t.usage.cache_read_tokens
                    agg["cache_write"] += t.usage.cache_creation_tokens
                    has_breakdown = True

            # If no per-turn breakdown, count as unclassified.
            if not has_breakdown and sa.total_tokens:
                agg["unclassified"] += sa.total_tokens

            # Recurse into nested sub-agents.
            child = _sum_subagent_tokens(sa.conversation)
            for k in agg:
                agg[k] += child[k]

    agg["total"] = agg["input"] + agg["output"] + agg["cache_read"] + agg["cache_write"] + agg["unclassified"]
    return agg


def _format_session_summary(s: Session, s_idx: int) -> str:
    parts = [f"S{s_idx} {s.phase}:"]
    r = s.result
    if s.agent_type == "opencode":
        parts.append(f"[{s.data_source}]")
    parts.append(f"{len(s.conversation)} turns")
    sync_n, async_n = _count_subagents(s.conversation)
    if sync_n or async_n:
        if async_n:
            parts.append(f"{sync_n + async_n} sub-agents ({async_n} async)")
        else:
            parts.append(f"{sync_n} sub-agents")
    if s.compact_events:
        parts.append(f"{len(s.compact_events)} compactions")
    # Token totals: main session + classified sub-agent tokens.
    in_tok = 0
    out_tok = 0
    cache_r = 0
    cache_c = 0
    if r is not None and r.model_usage:
        for mu in r.model_usage.values():
            in_tok += mu.input_tokens
            out_tok += mu.output_tokens
            cache_r += mu.cache_read_tokens
            cache_c += mu.cache_creation_tokens
    sub_tok = _sum_subagent_tokens(s.conversation)
    in_tok += sub_tok["input"]
    out_tok += sub_tok["output"]
    cache_r += sub_tok["cache_read"]
    cache_c += sub_tok["cache_write"]
    # Main + classified sub-agent tokens.
    if in_tok or out_tok or cache_r or cache_c:
        parts.append(
            f"{_format_compact_int(in_tok + out_tok)} new tok "
            f"(in={_format_compact_int(in_tok)} out={_format_compact_int(out_tok)} "
            f"cache_r={_format_compact_int(cache_r)} "
            f"cache_c={_format_compact_int(cache_c)})"
        )
    # Unclassified sub-agent tokens (Claude: no per-turn breakdown available).
    if sub_tok["unclassified"] > 0:
        parts.append(
            f"+{_format_compact_int(sub_tok['unclassified'])} sub-agent tok (mixed)"
        )
    # Use the timestamp-derived wall clock — the only field that matches
    # actual elapsed time. Frame-side `duration_ms` is broken for async
    # sub-agents (only counts main-agent activity, missing idle wait), and
    # `duration_api_ms` double-counts parallel sub-agent work.
    # If parallel work compressed real time noticeably (api_ms > 1.3× wall),
    # also report api_ms as a secondary "parallel work" figure.
    if s.wall_clock_ms:
        parts.append(_format_duration(s.wall_clock_ms))
        if r is not None and r.duration_api_ms > s.wall_clock_ms * 13 // 10:
            parts.append(
                f"({_format_duration(r.duration_api_ms)} api, parallelized)"
            )
    elif r is not None and r.duration_ms:
        parts.append(_format_duration(r.duration_ms))
    if r is not None and r.total_cost_usd:
        cost_label = f"${r.total_cost_usd:.2f}"
        if sub_tok["unclassified"] > 0:
            cost_label += " (main only)"
        parts.append(cost_label)
    if r is not None and r.is_error:
        parts.append(f"⚠ stop={r.stop_reason}")
    # Stall + frozen-task warnings: the session data alone under-reports wall
    # time when the process hung after its last activity (see _stall_gap_ms).
    stall_ms = _stall_gap_ms(s)
    if stall_ms:
        parts.append(
            f"⚠ stalled {_format_duration(stall_ms)} after last activity "
            f"(process ran {_format_duration(s.process_wall_ms)})"
        )
    frozen_n = _count_frozen_tools(s.conversation)
    if frozen_n:
        parts.append(f"⚠ {frozen_n} frozen task(s) never completed")
    return "   ".join(parts)


def render_timeline_svg(sessions: list[Session]) -> str:
    rows, groups = _flatten_rows(sessions)
    if not rows:
        return '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20"/>'

    max_total = max(
        (sum(seg.size for seg in row.segments) for row in rows), default=1
    ) or 1
    max_depth = max((row.depth for row in rows), default=0)
    n_turn_rows = sum(1 for row in rows if row.compact is None and not row.spacer)

    row_h = 14
    label_w = 90
    bar_w = 1400
    indent_px = 24
    bar_inset = 8   # gap between the group guide line and the start of bars
    pad_top = 90
    pad_bottom = 30

    # Pre-compute the y-offset and height of each row, so the renderer can
    # support variable-height rows (e.g. thin sub-agent spacers).
    row_y: list[int] = []
    row_heights: list[int] = []
    cursor = pad_top
    for row in rows:
        row_y.append(cursor)
        h = row.height if row.height is not None else row_h
        row_heights.append(h)
        cursor += h
    total_rows_height = cursor - pad_top
    grand_total_extra = (row_h + 12) if len(sessions) > 1 else 0
    height = pad_top + total_rows_height + grand_total_extra + pad_bottom
    width = label_w + max_depth * indent_px + bar_inset + bar_w + 40

    out: list[str] = []
    out.append('<?xml version="1.0" encoding="UTF-8"?>')
    out.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" '
        f'width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" '
        f'font-family="ui-monospace, monospace" font-size="11">'
    )
    out.append(f'<rect width="{width}" height="{height}" fill="{_BG}"/>')

    title = f"Trace timeline — {n_turn_rows} turns"
    out.append(
        f'<text x="20" y="28" font-size="16" font-weight="bold" fill="{_TITLE}">'
        f'{html.escape(title)}</text>'
    )

    legend_x = 20
    for cat, color in CAT_COLORS.items():
        out.append(
            f'<rect x="{legend_x}" y="50" width="14" height="14" '
            f'fill="{color}"/>'
        )
        out.append(
            f'<text x="{legend_x + 20}" y="62" fill="{_TEXT}">{cat}</text>'
        )
        legend_x += 80

    # X-axis tick marks (rough scale guide). Anchored at where depth-0 bars
    # actually start (label_w + bar_inset), so the ticks line up with the
    # left edge of the bar area.
    tick_origin = label_w + bar_inset
    for frac in (0.25, 0.5, 0.75, 1.0):
        x = tick_origin + bar_w * frac
        out.append(
            f'<line x1="{x}" y1="{pad_top - 6}" x2="{x}" y2="{height - pad_bottom}" '
            f'stroke="{_GRID}" stroke-dasharray="2,3"/>'
        )
        out.append(
            f'<text x="{x}" y="{pad_top - 10}" text-anchor="middle" '
            f'fill="{_TEXT_DIM}" font-size="10">{int(max_total * frac):,}</text>'
        )

    compact_color = "#c878d8"  # magenta-ish; stands out on dark bg

    # Sub-agent group guide lines: a purple vertical stroke spanning all
    # rows that belong to one sub-agent dispatch. Sits in the indent gutter
    # at the column where bars would have started without `bar_inset`,
    # `bar_inset` pixels to the left of the actual bars. Hovering the line
    # surfaces the same tooltip as the parent's purple Agent bar segment.
    group_color = CAT_COLORS.get(CAT_SUBAGENT, "#9b80c8")
    # Unfinished sub-agents keep the normal purple but the guide line fades
    # out toward the bottom ("the thread was never closed") and ends in a
    # small glyph: ✕ = still running when the process ended (killed),
    # ○ = trace has no result event yet (session may still be live).
    # Red is deliberately not used — it already means build/error elsewhere.
    fade_defs: list[str] = []
    for grp_i, grp in enumerate(groups):
        gx = label_w + grp.depth * indent_px + 3
        gy1 = row_y[grp.start_idx] + 1
        gy2 = row_y[grp.end_idx] + row_heights[grp.end_idx] - 1
        # "Never completed" evidence: an export that caught the tool still
        # running (tu.frozen) or a sub-agent status stuck at running/pending.
        # Either is only FROZEN once the session actually ended; in a live
        # trace it just means the sub-agent is still running right now.
        unfinished = grp.tool_use is not None and (
            grp.tool_use.frozen
            or (
                grp.tool_use.subagent is not None
                and grp.tool_use.subagent.status in ("running", "pending")
            )
        )
        frozen = unfinished and grp.session_ended
        in_flight = unfinished and not grp.session_ended
        tooltip_lines = []
        if grp.tool_use is not None:
            tu = grp.tool_use
            size = _tool_size(tu)
            desc = ""
            prompt_text = ""
            result_preview = ""
            if isinstance(tu.input, dict):
                desc = tu.input.get("description", "")
                prompt_text = tu.input.get("prompt", "")
            if tu.subagent:
                desc = desc or tu.subagent.description or ""
                prompt_text = prompt_text or tu.subagent.prompt or ""
            prompt_preview = prompt_text[:PREVIEW_SIZE].replace("\n", " ") if prompt_text else ""
            result_preview = (tu.result.content[:PREVIEW_SIZE] if tu.result else "").replace("\n", " ")
            if frozen:
                tooltip_lines.append(
                    "⚠ FROZEN — still running when the process ended"
                )
            elif in_flight:
                tooltip_lines.append(
                    "⏳ in progress — trace has no result event yet (live session)"
                )
            tooltip_lines.append(f"{CAT_SUBAGENT}: {size:,} chars")
            tooltip_lines.append(f"[task] {desc}")
            tooltip_lines.append(f"[prompt] {prompt_preview}")
            tooltip_lines.append(f"[result] {result_preview}")
        tooltip = html.escape("\n".join(tooltip_lines)) if tooltip_lines else ""
        # Async sub-agents: dashed line, signaling that the row order is
        # only approximate (synthesized from progress snapshots, not a true
        # API conversation).
        dash_attr = ' stroke-dasharray="5,3"' if grp.is_async else ''
        if unfinished:
            grad_id = f"gfade{grp_i}"
            fade_defs.append(
                f'<linearGradient id="{grad_id}" gradientUnits="userSpaceOnUse" '
                f'x1="0" y1="{gy1}" x2="0" y2="{gy2}">'
                f'<stop offset="55%" stop-color="{group_color}" stop-opacity="1"/>'
                f'<stop offset="100%" stop-color="{group_color}" stop-opacity="0"/>'
                f'</linearGradient>'
            )
            stroke = f"url(#{grad_id})"
        else:
            stroke = group_color
        parts = [
            f'<line x1="{gx}" y1="{gy1}" x2="{gx}" y2="{gy2}" '
            f'stroke="{stroke}" stroke-width="3"{dash_attr}/>'
        ]
        if frozen:
            # Thick stroked ✕ at the line end (a text glyph renders too thin).
            cy = gy2 - 3
            parts.append(
                f'<path d="M {gx - 4} {cy - 4} L {gx + 4} {cy + 4} '
                f'M {gx - 4} {cy + 4} L {gx + 4} {cy - 4}" '
                f'stroke="{group_color}" stroke-width="2.5" '
                f'stroke-linecap="round" fill="none"/>'
            )
        elif in_flight:
            # Three chevrons pointing down ("still flowing"), drawn over the
            # faded tail of the guide line.
            chevrons = []
            for k in range(3):
                y0 = gy2 - 14 + k * 5
                chevrons.append(
                    f'<path d="M {gx - 4} {y0} L {gx} {y0 + 4} L {gx + 4} {y0}" '
                    f'stroke="{group_color}" stroke-width="2.5" '
                    f'stroke-linecap="round" fill="none"/>'
                )
            parts.extend(chevrons)
        if tooltip:
            out.append(f'<g><title>{tooltip}</title>{"".join(parts)}</g>')
        else:
            out.append("".join(parts))

    if fade_defs:
        out.append("<defs>" + "".join(fade_defs) + "</defs>")

    for idx, row in enumerate(rows):
        y = row_y[idx]
        rh = row_heights[idx]

        # Spacer: blank vertical gap between row groups (sub-agent siblings
        # use a thin one; sessions use a full-row one). Draws nothing.
        if row.spacer:
            continue

        # Session summary banner: end-of-session stats (turns, sub-agents,
        # compactions, tokens, duration, cost). Spans the full width.
        if row.summary_text is not None:
            band_x = label_w
            band_w = width - band_x - 40
            out.append(
                f'<rect x="{band_x}" y="{y}" width="{band_w}" '
                f'height="{rh}" fill="#2a2f3a" stroke="{_GRID}" stroke-width="0.5"/>'
            )
            out.append(
                f'<text x="{band_x + 8}" y="{y + 11}" '
                f'fill="{_TITLE}" font-size="11" font-weight="bold">'
                f'{html.escape(row.summary_text)}</text>'
            )
            continue

        # Compaction divider: full-width band with a label.
        if row.compact is not None:
            ev = row.compact
            band_x = label_w
            band_w = width - band_x - 40
            cy = y + row_h / 2
            token_line = (
                f"{ev.pre_tokens:,} → {ev.post_tokens:,} tokens "
                f"({100 * ev.post_tokens / max(ev.pre_tokens, 1):.0f}%)"
                if ev.pre_tokens or ev.post_tokens else
                "token counts unavailable"
            )
            tooltip = html.escape(
                f"context compaction ({ev.trigger})\n"
                f"{token_line}\n"
                f"duration: {ev.duration_ms / 1000:.1f}s"
            )
            token_label = (
                f"{ev.pre_tokens // 1000}k → {ev.post_tokens // 1000}k tok"
                if ev.pre_tokens or ev.post_tokens else
                "tokens n/a"
            )
            label = f"⌁ compact: {token_label} ({ev.duration_ms / 1000:.0f}s, {ev.trigger})"
            out.append(
                f'<g><title>{tooltip}</title>'
                f'<line x1="{band_x}" y1="{cy:.1f}" x2="{band_x + band_w}" y2="{cy:.1f}" '
                f'stroke="{compact_color}" stroke-width="1" stroke-dasharray="4,3"/>'
                f'<text x="{band_x + 10}" y="{cy + 4:.1f}" '
                f'fill="{compact_color}" font-size="10">{html.escape(label)}</text>'
                f'</g>'
            )
            continue

        # `gutter_x` is where the group guide line lives; bars start
        # `bar_inset` pixels further to the right.
        gutter_x = label_w + row.depth * indent_px
        bar_start = gutter_x + bar_inset
        if idx % 2 == 0:
            # Start the zebra stripe at bar_start (not gutter_x) so the
            # `bar_inset` gap stays clear and doesn't paint over the
            # purple group guide line.
            out.append(
                f'<rect x="{bar_start}" y="{y}" '
                f'width="{width - bar_start - 40}" '
                f'height="{row_h}" fill="{_ROW_ALT}"/>'
            )
        label_fill = _TEXT_DIM if row.depth > 0 else _TEXT
        out.append(
            f'<text x="{gutter_x - 5}" y="{y + 11}" text-anchor="end" '
            f'fill="{label_fill}">{html.escape(row.label)}</text>'
        )
        x = float(bar_start)
        seg_gap = 2.0  # px gap between adjacent segments so equal-color blocks read separately
        for i, seg in enumerate(row.segments):
            w = max(seg.size / max_total * bar_w, 0.5)
            color = CAT_COLORS.get(seg.category, "#999")
            tooltip = html.escape(
                f"{seg.category}: {seg.size:,} chars\n{seg.tooltip}"
            )
            out.append(
                f'<g><title>{tooltip}</title>'
                f'<rect x="{x:.2f}" y="{y + 1}" width="{w:.2f}" '
                f'height="{row_h - 2}" fill="{color}"/></g>'
            )
            x += w
            if i < len(row.segments) - 1:
                x += seg_gap

    # Grand total across all sessions.
    if len(sessions) > 1:
        grand_in = 0
        grand_out = 0
        grand_cache_r = 0
        grand_cache_c = 0
        grand_unc = 0
        grand_cost = 0.0
        for s in sessions:
            r = s.result
            if r is not None and r.model_usage:
                for mu in r.model_usage.values():
                    grand_in += mu.input_tokens
                    grand_out += mu.output_tokens
                    grand_cache_r += mu.cache_read_tokens
                    grand_cache_c += mu.cache_creation_tokens
            if r is not None:
                grand_cost += r.total_cost_usd
            sub = _sum_subagent_tokens(s.conversation)
            grand_in += sub["input"]
            grand_out += sub["output"]
            grand_cache_r += sub["cache_read"]
            grand_cache_c += sub["cache_write"]
            grand_unc += sub["unclassified"]
        grand_y = cursor + 6
        grand_text = (
            f"Grand total: "
            f"{_format_compact_int(grand_in + grand_out)} new tok "
            f"(in={_format_compact_int(grand_in)} out={_format_compact_int(grand_out)} "
            f"cache_r={_format_compact_int(grand_cache_r)} "
            f"cache_c={_format_compact_int(grand_cache_c)})"
        )
        if grand_unc > 0:
            grand_text += f"   +{_format_compact_int(grand_unc)} sub-agent tok (mixed)"
        if grand_cost > 0:
            grand_cost_label = f"   ${grand_cost:.2f}"
            has_unclassified = any(
                _sum_subagent_tokens(s.conversation)["unclassified"] > 0 for s in sessions
            )
            if has_unclassified:
                grand_cost_label += " (main only)"
            grand_text += grand_cost_label
        band_x = label_w
        band_w = width - band_x - 40
        out.append(
            f'<rect x="{band_x}" y="{grand_y}" width="{band_w}" '
            f'height="{row_h}" fill="#2a2f3a" stroke="{_GRID}" stroke-width="0.5"/>'
        )
        out.append(
            f'<text x="{band_x + 8}" y="{grand_y + 11}" '
            f'fill="{_TITLE}" font-size="11" font-weight="bold">'
            f'{html.escape(grand_text)}</text>'
        )

    out.append('</svg>')
    return "\n".join(out)


# ---------------------------------------------------------------------------
# Format detection & OpenCode parser
# ---------------------------------------------------------------------------

_OPENCODE_EVENT_TYPES = {"step_start", "step_finish", "reasoning", "text", "tool_use"}


def _detect_format(records: list[tuple[int, dict]]) -> str:
    """Peek at the first few JSON records and return 'opencode' or 'claude'.

    OpenCode traces always contain `step_start` / `step_finish` events.
    Claude traces use `type=assistant` / `type=user` / `type=system`.
    """
    for _, obj in records[:20]:
        typ = obj.get("type", "")
        if typ in ("step_start", "step_finish", "reasoning"):
            return "opencode"
        if typ in ("assistant", "user", "system", "result"):
            return "claude"
    return "claude"


class OpenCodeParser:
    """Parse an OpenCode --format=json event stream into Session objects.

    OpenCode emits a flat JSONL stream:
      step_start → reasoning? → text? → tool_use* → step_finish

    Each step_start/step_finish pair becomes one Turn.
    tool_use events carry both input AND output inside `part.state`.
    Token/cost totals arrive in step_finish.
    """

    def parse_file(self, path: str) -> list[Session]:
        records: list[tuple[int, dict]] = []
        with open(path, "r") as f:
            for lineno, line in enumerate(f, 1):
                stripped = line.strip()
                if stripped.startswith("{"):
                    try:
                        records.append((lineno, json.loads(stripped)))
                    except json.JSONDecodeError:
                        pass

        by_session: dict[str, list[tuple[int, dict]]] = defaultdict(list)
        for lineno, obj in records:
            sid = obj.get("sessionID", "unknown")
            by_session[sid].append((lineno, obj))

        sessions = []
        for idx, (sid, events) in enumerate(by_session.items()):
            phases = ["translation", "verification", "unknown"]
            phase = phases[min(idx, 2)]
            sessions.append(self._parse_session(sid, phase, events))
        return sessions

    def _parse_session(
        self, session_id: str, phase: str, events: list[tuple[int, dict]]
    ) -> Session:
        session = Session(session_id=session_id, phase=phase, agent_type="opencode", data_source="jsonl")
        session.wall_clock_ms = _wall_clock_ms(events)

        current_turn: Optional[Turn] = None
        turn_index = 0
        accumulated_usage = TokenUsage(0, 0, 0, 0, 0)
        step_cost = 0.0

        for lineno, obj in events:
            typ = obj.get("type", "")
            part = obj.get("part", {})

            if typ == "step_start":
                turn_index += 1
                current_turn = Turn(turn_index=turn_index)
                continue

            if typ == "reasoning" and current_turn is not None:
                text = part.get("text", "")
                if text:
                    current_turn.content_blocks.append(
                        ContentBlock(type="thinking", thinking=text)
                    )
                continue

            if typ == "text" and current_turn is not None:
                text = part.get("text", "")
                if text:
                    current_turn.content_blocks.append(
                        ContentBlock(type="text", text=text)
                    )
                continue

            if typ == "tool_use" and current_turn is not None:
                tool_name = part.get("tool", "unknown")
                state = part.get("state", {})
                inp = state.get("input") or {}
                if not isinstance(inp, dict):
                    inp = {"input": inp}
                output = state.get("output")
                status = state.get("status", "")
                call_id = part.get("callID", f"oc-{lineno}")

                result = None
                if output is not None or status:
                    result = ToolResult(
                        tool_use_id=call_id,
                        content=str(output) if output is not None else "",
                        is_error=status in ("error", "failed"),
                    )

                tu = ToolUse(id=call_id, name=tool_name, input=inp, result=result)
                current_turn.content_blocks.append(
                    ContentBlock(type="tool_use", tool_use=tu)
                )
                continue

            if typ == "step_finish" and current_turn is not None:
                tokens = part.get("tokens", {})
                cache = tokens.get("cache", {})
                # Use the explicit `input` field from step_finish, NOT
                # `total - output - reasoning`.  The `total` field includes
                # cache_read tokens, so subtracting output/reasoning from it
                # inflates input by the cache_read amount.  The `input` field
                # already contains only the true non-cached input tokens.
                in_tok = tokens.get("input", 0)
                # OpenCode reports reasoning separately from output, but the
                # provider bills reasoning at the output rate — fold it in so
                # output_tokens is directly usable for cost estimation.
                reason_tok = tokens.get("reasoning", 0)
                out_tok = tokens.get("output", 0) + reason_tok
                turn_usage = TokenUsage(
                    input_tokens=in_tok,
                    output_tokens=out_tok,
                    cache_creation_tokens=cache.get("write", 0),
                    cache_read_tokens=cache.get("read", 0),
                    reasoning_tokens=reason_tok,
                )
                current_turn.usage = turn_usage

                cost = part.get("cost")
                if cost is not None:
                    step_cost += float(cost)

                accumulated_usage = TokenUsage(
                    input_tokens=accumulated_usage.input_tokens + turn_usage.input_tokens,
                    output_tokens=accumulated_usage.output_tokens + turn_usage.output_tokens,
                    cache_creation_tokens=accumulated_usage.cache_creation_tokens + turn_usage.cache_creation_tokens,
                    cache_read_tokens=accumulated_usage.cache_read_tokens + turn_usage.cache_read_tokens,
                    reasoning_tokens=accumulated_usage.reasoning_tokens + turn_usage.reasoning_tokens,
                )

                session.conversation.append(current_turn)
                current_turn = None
                continue

        session.result = ResultEvent(
            is_error=False,
            stop_reason="end",
            num_turns=len(session.conversation),
            duration_ms=session.wall_clock_ms,
            duration_api_ms=0,
            total_cost_usd=step_cost if step_cost > 0 else 0.0,
            result_text="",
            model_usage={},
        )
        return session


class OpenCodeExportParser:
    """Parse an OpenCode export JSON (from `opencode export <sessionID>`) into a Session.

    Export format has `info` (session metadata) and `messages[]` (conversation).
    Each message has `parts[]` with types: step-start, reasoning, text, tool, step-finish.
    """

    def parse_export(self, export_json: dict) -> Session:
        info = export_json.get("info", {})
        session_id = info.get("id", "unknown")
        title = info.get("title", "")
        agent_type = "opencode"
        phase = "unknown"
        if "translate" in title.lower() or "translate" in info.get("agent", "").lower():
            phase = "translation"
        elif "verify" in title.lower() or "verify" in info.get("agent", "").lower():
            phase = "verification"

        session = Session(session_id=session_id, phase=phase, agent_type=agent_type, data_source="export")

        # Extract wall-clock from info.time
        time_info = info.get("time", {})
        created = time_info.get("created")
        updated = time_info.get("updated")
        if isinstance(created, (int, float)) and isinstance(updated, (int, float)):
            session.wall_clock_ms = max(int(updated - created), 0)

        # Parse messages into turns
        messages = export_json.get("messages", [])
        current_turn: Optional[Turn] = None
        turn_index = 0
        step_cost = 0.0

        for msg in messages:
            msg_info = msg.get("info", {})
            parts = msg.get("parts", [])

            for part in parts:
                ptype = part.get("type", "")

                if ptype == "step-start":
                    turn_index += 1
                    current_turn = Turn(turn_index=turn_index)
                    continue

                if ptype == "reasoning" and current_turn is not None:
                    text = part.get("text", "")
                    if text:
                        current_turn.content_blocks.append(
                            ContentBlock(type="thinking", thinking=text)
                        )
                    continue

                if ptype == "text" and current_turn is not None:
                    text = part.get("text", "")
                    if text:
                        current_turn.content_blocks.append(
                            ContentBlock(type="text", text=text)
                        )
                    continue

                if ptype == "compaction":
                    trigger = "auto" if part.get("auto") else "manual"
                    if part.get("overflow"):
                        trigger = f"{trigger}/overflow"
                    session.compact_events.append(CompactEvent(
                        pre_tokens=0,
                        post_tokens=0,
                        duration_ms=0,
                        trigger=trigger,
                        line_number=0,
                        after_turn_index=len(session.conversation),
                    ))
                    continue

                if ptype == "tool" and current_turn is not None:
                    tool_name = part.get("tool", "unknown")
                    state = part.get("state", {})
                    inp = state.get("input") or {}
                    if not isinstance(inp, dict):
                        inp = {"input": inp}
                    output = state.get("output")
                    status = state.get("status", "")
                    call_id = part.get("callID", f"export-{turn_index}")

                    # A tool still "running"/"pending" in a post-mortem export
                    # never completed: the agent process ended (usually killed
                    # by the harness timeout) while this call was in flight.
                    frozen = status in ("running", "pending")

                    result = None
                    if frozen:
                        result = ToolResult(
                            tool_use_id=call_id,
                            content=f"[FROZEN: still '{status}' at export time — "
                                    "never completed before the process ended]",
                            is_error=True,
                        )
                    elif output is not None or status:
                        result = ToolResult(
                            tool_use_id=call_id,
                            content=str(output) if output is not None else "",
                            is_error=status in ("error", "failed"),
                        )

                    tu = ToolUse(
                        id=call_id, name=tool_name, input=inp,
                        result=result, frozen=frozen,
                    )
                    current_turn.content_blocks.append(
                        ContentBlock(type="tool_use", tool_use=tu)
                    )
                    continue

                if ptype == "step-finish" and current_turn is not None:
                    tokens = part.get("tokens", {})
                    cache = tokens.get("cache", {})
                    # Use the explicit `input` field, NOT `total - output -
                    # reasoning`.  The `total` field includes cache_read, so
                    # the subtraction would inflate input by the cache_read amount.
                    in_tok = tokens.get("input", 0)
                    # Fold reasoning into output: billed at the output rate
                    # (see TokenUsage docstring).
                    reason_tok = tokens.get("reasoning", 0)
                    out_tok = tokens.get("output", 0) + reason_tok
                    current_turn.usage = TokenUsage(
                        input_tokens=in_tok,
                        output_tokens=out_tok,
                        cache_creation_tokens=cache.get("write", 0),
                        cache_read_tokens=cache.get("read", 0),
                        reasoning_tokens=reason_tok,
                    )
                    cost = part.get("cost")
                    if cost is not None:
                        step_cost += float(cost)
                    session.conversation.append(current_turn)
                    current_turn = None
                    continue

        # Flush a trailing unfinished turn. A session that ended mid-flight
        # (stall / timeout kill) has a final message with no step-finish;
        # dropping it would hide exactly the frozen tool calls we want to see.
        if current_turn is not None and current_turn.content_blocks:
            session.conversation.append(current_turn)
            current_turn = None

        # Build result from info-level aggregation
        info_tokens = info.get("tokens", {})
        info_cost = info.get("cost", 0.0)
        model_info = info.get("model", {})
        model_id = model_info.get("id", "unknown")
        model_usage = {}
        if info_tokens:
            # Fold reasoning into output: billed at the output rate
            # (see TokenUsage docstring).
            reason_tok = int(info_tokens.get("reasoning", 0))
            model_usage[model_id] = ModelUsage(
                model=model_id,
                input_tokens=int(info_tokens.get("input", 0)),
                output_tokens=int(info_tokens.get("output", 0)) + reason_tok,
                cache_read_tokens=int(info_tokens.get("cache", {}).get("read", 0)),
                cache_creation_tokens=int(info_tokens.get("cache", {}).get("write", 0)),
                cost_usd=float(info_cost) if info_cost else 0.0,
                reasoning_tokens=reason_tok,
            )
        session.result = ResultEvent(
            is_error=False,
            stop_reason="end",
            num_turns=len(session.conversation),
            duration_ms=session.wall_clock_ms,
            duration_api_ms=0,
            total_cost_usd=float(info_cost) if info_cost else 0.0,
            result_text="",
            model_usage=model_usage,
        )
        return session


def _read_mixed_trace(
    path: str,
) -> tuple[dict[str, dict], dict[str, list[tuple[int, dict]]]]:
    """Read a mixed-format trace file, separating export blocks from JSONL events.

    Returns:
        exports: session_id -> export JSON dict
        jsonl_events: session_id -> list of (lineno, event_dict)
    """
    exports: dict[str, dict] = {}
    jsonl_events: dict[str, list[tuple[int, dict]]] = defaultdict(list)

    with open(path, "r") as f:
        lines = f.readlines()

    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        stripped = line.strip()

        # Skip empty / non-JSON lines (ANSI logs, benchmark output, markers)
        if not stripped:
            i += 1
            continue

        # Detect export block: a multi-line JSON object with "info" + "messages".
        # Single-line JSONL events are parsed immediately; multi-line export
        # blocks are accumulated until json.loads succeeds.
        if stripped.startswith("{"):
            # Try single-line parse first (covers all JSONL events).
            try:
                obj = json.loads(stripped, strict=False)
            except json.JSONDecodeError:
                obj = None

            if obj is not None:
                # Single-line JSON: classify immediately.
                if "info" in obj and "messages" in obj:
                    sid = obj.get("info", {}).get("id", "unknown")
                    exports[sid] = obj
                elif "type" in obj:
                    sid = obj.get("sessionID", "unknown")
                    jsonl_events[sid].append((i + 1, obj))
                i += 1
                continue

            # Multi-line JSON: accumulate lines until json.loads succeeds.
            # Optimization: only attempt parsing when the line ends with '}'
            # (likely end of a JSON object), avoiding O(n²) parse attempts.
            buf_lines = [line]
            j = i + 1
            parsed_obj = None
            while j < n:
                buf_lines.append(lines[j])
                if lines[j].rstrip().endswith("}"):
                    buf = "".join(buf_lines).strip()
                    try:
                        parsed_obj = json.loads(buf, strict=False)
                        break
                    except json.JSONDecodeError:
                        pass
                j += 1

            if parsed_obj is not None:
                if "info" in parsed_obj and "messages" in parsed_obj:
                    sid = parsed_obj.get("info", {}).get("id", "unknown")
                    exports[sid] = parsed_obj
                elif "type" in parsed_obj:
                    sid = parsed_obj.get("sessionID", "unknown")
                    jsonl_events[sid].append((i + 1, parsed_obj))
                i = j + 1
                continue

            # Could not parse as JSON; skip this line.
            i += 1
            continue

        # Non-JSON line; skip.
        i += 1

    return exports, jsonl_events


def _export_opencode_session_live(session_id: str, timeout_s: int = 30) -> Optional[dict]:
    """Try to export an OpenCode session directly from the local session store.

    This is used while a trace is still being written. The trace file may only
    contain live JSONL events because `agent_runner` appends `opencode export`
    blocks after `opencode run` exits. For visualization during an active run,
    pull the export by sessionID here and mark it as data_source="live-export".

    Important: OpenCode 1.16.x can truncate `opencode export` output when stdout
    is captured through a pipe. Mirror the Rust-side workaround: redirect stdout
    to a temporary file, then read that file.
    """
    with tempfile.NamedTemporaryFile(prefix="opencode-export-", suffix=".json") as tmp:
        try:
            result = subprocess.run(
                ["opencode", "export", session_id],
                stdout=tmp,
                stderr=subprocess.DEVNULL,
                timeout=timeout_s,
                check=False,
            )
        except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
            return None

        if result.returncode != 0:
            return None

        tmp.flush()
        tmp.seek(0)
        data = tmp.read().decode("utf-8", errors="replace")

    if not data.strip():
        return None
    try:
        obj = json.loads(data, strict=False)
    except json.JSONDecodeError:
        return None
    if "info" not in obj or "messages" not in obj:
        return None
    return obj


def _opencode_session_parent_id(export_json: dict) -> str:
    """Return an OpenCode export's parent session ID, if it has one."""
    info = export_json.get("info", {}) or {}
    for key in ("parentID", "parentId", "parentSessionID", "parentSessionId", "parent_session_id"):
        sid = info.get(key)
        if isinstance(sid, str) and sid:
            return sid
    return ""


def _opencode_task_child_session_id(part: dict) -> str:
    """Return the child session ID created by an OpenCode task tool part."""
    state = part.get("state", {}) or {}
    metadata = state.get("metadata", {}) or part.get("metadata", {}) or {}
    for key in ("sessionID", "sessionId", "session_id"):
        sid = metadata.get(key)
        if isinstance(sid, str) and sid:
            return sid
    return ""


def _extract_opencode_sub_session_ids(export_json: dict) -> list[str]:
    """Return child session IDs referenced by OpenCode task tool calls."""
    found: list[str] = []
    seen: set[str] = set()

    for msg in export_json.get("messages", []):
        for part in msg.get("parts", []):
            tool_name = str(part.get("tool", "")).lower()
            if part.get("type") != "tool" or tool_name != "task":
                continue

            sid = _opencode_task_child_session_id(part)
            if sid and sid not in seen:
                seen.add(sid)
                found.append(sid)

    return found


def _find_tool_use_by_id(turns: list[Turn], tool_use_id: str) -> Optional[ToolUse]:
    """Find a tool call by ID, descending into already-linked subagents."""
    for turn in turns:
        for tu in turn.tool_uses:
            if tu.id == tool_use_id:
                return tu
            if tu.subagent is not None:
                found = _find_tool_use_by_id(tu.subagent.conversation, tool_use_id)
                if found is not None:
                    return found
    return None


def _opencode_info_token_total(info: dict) -> int:
    tokens = info.get("tokens", {}) or {}
    cache = tokens.get("cache", {}) or {}
    return int(tokens.get("input", 0) or 0) + int(tokens.get("output", 0) or 0) + int(tokens.get("reasoning", 0) or 0) + int(cache.get("read", 0) or 0) + int(cache.get("write", 0) or 0)


def _attach_opencode_child_sessions(
    export_jsons: dict[str, dict],
    parsed_sessions: dict[str, Session],
) -> set[str]:
    """Attach exported OpenCode child sessions to their parent task ToolUse.

    OpenCode stdout JSONL only shows a `task` tool result in the parent session;
    the child conversation lives in a separate `opencode export <childSessionId>`.
    Once we have both exports, represent the child as `ToolUse.subagent` so the
    existing SVG flattener draws nested rows and purple sub-agent guide lines,
    instead of rendering child sessions as unrelated top-level S1/S2/... blocks.
    """
    child_sids: set[str] = set()

    # First pass: explicit parentID on child exports. This prevents child
    # sessions from being rendered top-level even if the parent task part is not
    # present yet in a live/incomplete export.
    for sid, export_json in export_jsons.items():
        parent_sid = _opencode_session_parent_id(export_json)
        if parent_sid and parent_sid in parsed_sessions:
            child_sids.add(sid)

    # Second pass: connect task ToolUse -> child SubAgent using task metadata.
    for parent_sid, export_json in export_jsons.items():
        parent_session = parsed_sessions.get(parent_sid)
        if parent_session is None:
            continue

        for msg in export_json.get("messages", []):
            for part in msg.get("parts", []):
                tool_name = str(part.get("tool", "")).lower()
                if part.get("type") != "tool" or tool_name != "task":
                    continue

                child_sid = _opencode_task_child_session_id(part)
                child_session = parsed_sessions.get(child_sid)
                if not child_sid or child_session is None:
                    continue

                call_id = part.get("callID", "")
                if not call_id:
                    continue
                task_tool = _find_tool_use_by_id(parent_session.conversation, call_id)
                if task_tool is None:
                    continue

                state = part.get("state", {}) or {}
                inp = state.get("input") or {}
                if not isinstance(inp, dict):
                    inp = {}
                child_info = export_jsons.get(child_sid, {}).get("info", {}) or {}
                task_tool.subagent = SubAgent(
                    task_id=child_sid,
                    tool_use_id=call_id,
                    description=inp.get("description", "") or child_info.get("title", ""),
                    prompt=inp.get("prompt", ""),
                    status=state.get("status", "completed"),
                    conversation=child_session.conversation,
                    is_async=False,
                    compact_events=child_session.compact_events,
                    total_tokens=_opencode_info_token_total(child_info),
                    total_tool_uses=sum(len(t.tool_uses) for t in child_session.conversation),
                    duration_ms=child_session.wall_clock_ms,
                )
                child_sids.add(child_sid)

    return child_sids


def _fetch_live_opencode_exports(
    jsonl_events: dict[str, list[tuple[int, dict]]],
    existing_exports: dict[str, dict],
) -> dict[str, dict]:
    """Fetch export JSON for JSONL sessions that lack in-file export blocks.

    Returns only successfully fetched exports. Failures are deliberately silent:
    the caller will fall back to JSONL for any session that cannot be exported.
    Child sessions discovered from task tool metadata are fetched breadth-first so
    live visualization can show sub-agent conversations before the run finishes.
    """
    live_exports: dict[str, dict] = {}
    queue = [sid for sid in jsonl_events if sid not in existing_exports]
    queued: set[str] = set(queue)

    while queue:
        sid = queue.pop(0)
        if sid in existing_exports or sid in live_exports:
            continue

        exported = _export_opencode_session_live(sid)
        if exported is None:
            continue

        live_exports[sid] = exported

        for child_sid in _extract_opencode_sub_session_ids(exported):
            if (
                child_sid not in existing_exports
                and child_sid not in live_exports
                and child_sid not in queued
            ):
                queue.append(child_sid)
                queued.add(child_sid)

    return live_exports


_ISO_TS_RE = re.compile(r"(20\d{2}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z)")


def _extract_runner_phase_windows(path: str) -> list[tuple[str, int, int]]:
    """Extract per-phase agent *process* windows from agent_runner log lines.

    The benchmark trace interleaves the agent's JSON stream with agent_runner
    tracing lines (ISO-8601 timestamps). The window from
    "Invoking OpenCode <phase> agent" to the first post-exit marker
    ("Exporting OpenCode session" / "Appended agent trace") is the real
    process lifetime — including any stall between the session's last
    activity and the harness timeout kill, which is invisible in the
    session's own data.

    Returns a list of (phase, start_ms, end_ms) in file order; windows with
    no end marker are omitted.
    """
    from datetime import datetime

    windows: list[tuple[str, int, int]] = []
    open_phase: Optional[str] = None
    open_start_ms = 0

    def _iso_ms(line: str) -> Optional[int]:
        m = _ISO_TS_RE.search(line)
        if not m:
            return None
        try:
            dt = datetime.fromisoformat(m.group(1).replace("Z", "+00:00"))
        except ValueError:
            return None
        return int(dt.timestamp() * 1000)

    try:
        with open(path, "r", errors="replace") as f:
            for line in f:
                if line.lstrip().startswith("{"):
                    continue  # JSON event/export content, not a runner log line
                if "Invoking OpenCode" in line and "agent" in line:
                    ts = _iso_ms(line)
                    if ts is None:
                        continue
                    phase = (
                        "translation" if "translation agent" in line
                        else "verification" if "verification agent" in line
                        else "unknown"
                    )
                    open_phase = phase
                    open_start_ms = ts
                elif open_phase is not None and (
                    "Exporting OpenCode session" in line
                    or "Appended agent trace" in line
                ):
                    ts = _iso_ms(line)
                    if ts is not None and ts >= open_start_ms:
                        windows.append((open_phase, open_start_ms, ts))
                    open_phase = None
    except OSError:
        return []

    return windows


def parse_mixed_trace_file(path: str, fmt: str = "auto") -> list[Session]:
    """Parse a trace file that may contain both JSONL events and export blocks.

    Priority per session:
      1. Export block in file -> use OpenCodeExportParser (data_source="export")
      2. Live `opencode export <sessionID>` -> use OpenCodeExportParser (data_source="live-export")
      3. JSONL events -> use OpenCodeParser (data_source="jsonl")
    """
    # Peek to determine the overall format (claude vs opencode).
    peek_records: list[tuple[int, dict]] = []
    with open(path, "r") as f:
        for lineno, line in enumerate(f, 1):
            if lineno > 200:
                break
            stripped = line.strip()
            if stripped.startswith("{"):
                try:
                    peek_records.append((lineno, json.loads(stripped)))
                    if len(peek_records) >= 5:
                        break
                except json.JSONDecodeError:
                    pass

    if fmt == "auto":
        fmt = _detect_format(peek_records)

    if fmt == "claude":
        return TraceParser().parse_file(path)

    # OpenCode path: read mixed file, prefer export blocks.
    exports, jsonl_events = _read_mixed_trace(path)
    live_exports = _fetch_live_opencode_exports(jsonl_events, exports)

    export_parser = OpenCodeExportParser()
    jsonl_parser = OpenCodeParser()
    sessions: list[Session] = []

    # Parse every available export first, then attach child exports beneath
    # parent `task` tool calls. Only root exports should remain top-level.
    export_sources: dict[str, str] = {}
    combined_exports: dict[str, dict] = {}
    for sid, export_json in exports.items():
        combined_exports[sid] = export_json
        export_sources[sid] = "export"
    for sid, export_json in live_exports.items():
        if sid in combined_exports:
            continue
        combined_exports[sid] = export_json
        export_sources[sid] = "live-export"

    parsed_exports: dict[str, Session] = {}
    for sid, export_json in combined_exports.items():
        session = export_parser.parse_export(export_json)
        session.data_source = export_sources.get(sid, "export")
        parsed_exports[sid] = session

    child_export_sids = _attach_opencode_child_sessions(combined_exports, parsed_exports)
    seen_sids: set[str] = set(parsed_exports)

    # Phase assignment: first root session = translation, second = verification.
    phase_counter = 0

    # First/second: in-file exports and live exports, but only root sessions.
    for sid in combined_exports:
        if sid in child_export_sids:
            continue
        session = parsed_exports[sid]
        session.phase = ["translation", "verification", "unknown"][min(phase_counter, 2)]
        phase_counter += 1
        sessions.append(session)

    # Third: parse remaining sessions from JSONL (only if no export for that sid).
    for sid, events in jsonl_events.items():
        if sid in seen_sids:
            continue
        session = jsonl_parser._parse_session(
            sid,
            ["translation", "verification", "unknown"][min(phase_counter, 2)],
            events,
        )
        phase_counter += 1
        sessions.append(session)

    # If we found nothing at all, fall back to the raw parsers.
    if not sessions:
        return OpenCodeParser().parse_file(path)

    # Attach agent-process wall time from agent_runner markers, so stall
    # time between the session's last activity and the process's death
    # (timeout kill) becomes visible. Match windows to root sessions by
    # phase, in file order.
    for phase, start_ms, end_ms in _extract_runner_phase_windows(path):
        for session in sessions:
            if session.process_wall_ms == 0 and (
                session.phase == phase or phase == "unknown"
            ):
                session.process_wall_ms = end_ms - start_ms
                break

    return sessions


def parse_trace_file(path: str, fmt: str = "auto") -> list[Session]:
    """Top-level entry point: auto-detect format and parse.

    For OpenCode traces, uses mixed-format parsing (export blocks take priority
    over JSONL events for the same session).
    """
    return parse_mixed_trace_file(path, fmt)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        print("Usage: parse_trace.py <file> [-v] [-r] [-f] [--readable] [--visualize] [--file-io] [--format auto|claude|opencode]")
        print("")
        print("  <file>          Path to the trace file to parse")
        print("  -v, --visualize Generate SVG timeline visualization")
        print("  -r, --readable  Generate human-readable text history")
        print("  -f, --file-io   Generate file I/O analysis JSON")
        print("  --format        Force trace format: auto (default), claude, or opencode")
        print("")
        print("If no output flags are given, prints session statistics.")
        sys.exit(0)

    path = args[0]

    # Parse flags: -v, -r, -f, --readable, --visualize, --file-io, --format
    flags: set[str] = set()
    fmt = "auto"
    i = 1
    while i < len(args):
        arg = args[i]
        if arg == "--format" and i + 1 < len(args):
            fmt = args[i + 1]
            i += 2
            continue
        if arg.startswith("--format="):
            fmt = arg.split("=", 1)[1]
            i += 1
            continue
        if arg.startswith("--"):
            flags.add(arg)
        elif arg.startswith("-"):
            for ch in arg[1:]:
                flags.add(ch)
        i += 1

    readable = "r" in flags or "--readable" in flags
    visualize = "v" in flags or "--visualize" in flags
    file_io = "f" in flags or "--file-io" in flags

    sessions = parse_trace_file(path, fmt)
    detected = sessions[0].agent_type if sessions else "unknown"
    print(f"Detected format: {detected}")

    if readable:
        out_path = path.rsplit(".", 1)[0] + "_readable.txt"
        content = build_readable_history(sessions)
        with open(out_path, "w") as f:
            f.write(content)
        print(f"Written to {out_path}  ({len(content):,} chars)")

    if visualize:
        out_path = path.rsplit(".", 1)[0] + "_timeline.svg"
        content = render_timeline_svg(sessions)
        with open(out_path, "w") as f:
            f.write(content)
        print(f"Written to {out_path}  ({len(content):,} chars)")

    if file_io:
        import json as _json
        out_path = path.rsplit(".", 1)[0] + "_file_io.json"
        report = generate_file_io_report(sessions)
        with open(out_path, "w") as f:
            _json.dump(report, f, indent=2)
        print(f"Written to {out_path}")

    if not readable and not visualize and not file_io:
        print_session_stats(sessions)
