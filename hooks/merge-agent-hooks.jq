# Merge Guardian hook entries into (or remove them from) an agent's JSON hook config:
# Claude Code settings.json, Codex hooks.json (entries nest handlers under .hooks) and
# Cursor hooks.json (entries are the handlers themselves).
#
# Usage: jq -s --arg root <hook-root> --arg mode merge  -f merge-agent-hooks.jq <existing> <guardian>
#        jq -s --arg root <hook-root> --arg mode remove -f merge-agent-hooks.jq <existing>
#
# A handler is Guardian's only if its command points into <hook-root>/ — the directory this
# agent's Guardian scripts are installed to (e.g. ~/.claude/hooks/guardian). A hook that merely
# lives in some other directory named hooks/guardian is not touched.
#
# Idempotent: Guardian handlers already present are removed first,
# so re-running the installer never duplicates them. Everything else in the file — other
# settings, other events, other people's hooks on the same events — is preserved untouched,
# including entries in shapes this filter does not understand.
# Shapes that cannot be merged safely raise an error instead of being rewritten.
# The root as it appears in a command: plain, or as the inside of a shell-quoted string.
def markers: [($root + "/"), (($root + "/") | @sh | .[1:-1])] | unique;

def is_guardian:
  if type != "object" then false
  else ((.command? // "") | tostring) as $c | any(markers[]; . as $m | $c | contains($m))
  end;

# Only entries that actually carry a Guardian handler are modified: a flat (Cursor-style)
# Guardian entry is dropped; a nested entry loses its Guardian handlers and is dropped only
# when that leaves it empty.
def without_guardian:
  map(
    if is_guardian then
      empty
    elif type == "object" and ((.hooks? | type) == "array") and any(.hooks[]; is_guardian) then
      (.hooks |= map(select(is_guardian | not)))
      | if (.hooks | length) == 0 then empty else . end
    else . end
  );

(.[0] // {}) as $base
| (.[1] // {"hooks": {}}) as $g
| if ($base | type) != "object" then
    error("the settings file is not a JSON object")
  elif ($base.hooks != null and ($base.hooks | type) != "object") then
    error("its \"hooks\" value is not an object")
  elif $mode == "remove" then
    $base
    | if (.hooks | type) == "object" then
        .hooks |= with_entries(
          if (.value | type) == "array" then
            (.value | without_guardian) as $kept
            # Drop an event only when removing Guardian is what emptied it.
            | if ($kept | length) == 0 and (.value | length) > 0 then empty else .value = $kept end
          else . end
        )
      else . end
  else
    # Top-level keys the template carries (Cursor's "version") are defaults; the user's win.
    (($g | del(.hooks)) + $base)
    | .hooks = (
        reduce ($g.hooks | keys_unsorted[]) as $ev (
          ($base.hooks // {});
          if (.[$ev] != null and (.[$ev] | type) != "array") then
            error("its hooks.\($ev) value is not an array")
          else
            .[$ev] = (((.[$ev] // []) | without_guardian) + $g.hooks[$ev])
          end
        )
      )
  end
