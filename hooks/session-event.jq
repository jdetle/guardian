# Builds the SQL that records one hook event in sessions.db. Used by session-event.sh.
#
# Input: the hook's JSON, as Cursor, Claude Code or Codex sends it.
# --arg event       prompt | tool | end
# --arg client      cursor | claude | codex
# --arg state_text  the contents of ~/.guardian/state.json, or "" if there is none
#
# Output: SQL statements, or nothing when the input names no session. Every value from the
# input is written as a quoted SQL string or a checked number, never spliced in as it came.

def text: if . == null then "" elif type == "string" then . else tostring end;
def q: text | gsub("\u0000"; "") | gsub("'"; "''") | "'" + . + "'";
def int: (if type == "number" then . elif type == "string" then (tonumber? // 0) else 0 end) | floor;
def num: if type == "number" then . else 0 end;

# Tools that start another agent: the fan-out Guardian is here to keep an eye on.
def spawns_agent: . == "Agent" or . == "Task" or . == "spawn_agent";

(if type == "object" then . else {} end) as $in
| ($in.conversation_id // $in.session_id | text) as $sid
| if $sid == "" then empty else
    ($sid | q) as $id
    | ($in.model | text) as $model
    | (($in.cwd // ($in.workspace_roots | if type == "array" then .[0] else null end)) | text) as $cwd
    | ($state_text | fromjson? // {} | if type == "object" then . else {} end) as $state
    | [
        "BEGIN IMMEDIATE;",
        # A session whose start was never seen (hooks installed while it was running) still
        # gets a row.
        "INSERT OR IGNORE INTO sessions (conversation_id, model, client, state) VALUES (\($id), \((if $model == "" then "unknown" else $model end) | q), \($client | q), 'idle');",
        "UPDATE sessions SET last_activity_at = datetime('now'), client = COALESCE(client, \($client | q))"
          + (if $cwd != "" then ", cwd = \($cwd | q)" else "" end)
          + (if $model != "" then ", model = CASE WHEN model IN ('', 'unknown') THEN \($model | q) ELSE model END" else "" end)
          + " WHERE conversation_id = \($id);"
      ]
      + (
        if $event == "prompt" then
          ["UPDATE sessions SET state = 'working', prompt_count = COALESCE(prompt_count, 0) + 1 WHERE conversation_id = \($id);"]
        elif $event == "tool" then
          ($in.tool_name | text | if . == "" then "unknown" else . end) as $tool
          | [
              "INSERT INTO tool_calls (conversation_id, tool_name, duration_ms, model) VALUES (\($id), \($tool | q), \($in.duration_ms // $in.duration | int), \((if $model == "" then "unknown" else $model end) | q));",
              "UPDATE sessions SET state = 'working', tool_call_count = COALESCE(tool_call_count, 0) + 1"
                + (if ($tool | spawns_agent) then ", subagent_count = COALESCE(subagent_count, 0) + 1" else "" end)
                + " WHERE conversation_id = \($id);"
            ]
            # A pressure sample, only from a state.json written in this minute or the last:
            # an old file says nothing about the load this tool call ran under.
            + (
              ($state.sampled_at | text | .[0:16]) as $minute
              | ($state.pressure | text) as $pressure
              | if ($minute == (now | strftime("%Y-%m-%dT%H:%M")) or $minute == (now - 60 | strftime("%Y-%m-%dT%H:%M")))
                   and ($pressure == "clear" or $pressure == "strained" or $pressure == "critical")
                then ["INSERT INTO pressure_samples (conversation_id, pressure, cpu_percent, memory_available_gb, docker_cpu_percent) VALUES (\($id), \($pressure | q), \($state.cpu_percent | num), \($state.memory_available_gb | num), \($state.docker.total_cpu_percent? | num));"]
                else [] end
            )
        elif $event == "end" then
          ($in.reason // $in.final_status | text) as $why
          | ["UPDATE sessions SET state = 'ended', ended_at = datetime('now'), duration_ms = CAST((julianday('now') - julianday(started_at)) * 86400000 AS INTEGER)"
              + (if $why != "" then ", status = CASE WHEN status IS NULL OR status IN ('', 'unknown') THEN \($why | q) ELSE status END" else "" end)
              + " WHERE conversation_id = \($id);"]
        else [] end
      )
      + ["COMMIT;"]
    | join("\n")
  end
