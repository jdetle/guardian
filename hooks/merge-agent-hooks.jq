# Merge Guardian hook entries into a Claude Code settings.json or a Codex hooks.json.
# Usage: jq -s --arg marker "/hooks/guardian/" -f merge-agent-hooks.jq <existing> <guardian>
#
# Idempotent: Guardian handlers already present (command contains $marker) are removed first,
# so re-running the installer never duplicates them. Everything else in the file — other
# settings, other events, other people's hooks on the same events — is preserved untouched,
# including entries in shapes this filter does not understand.
# Shapes that cannot be merged safely raise an error instead of being rewritten.
def is_guardian:
  type == "object" and ((.command? // "") | tostring | contains($marker));

# Only entries that actually carry a Guardian handler are modified; an entry is dropped only
# when removing Guardian's handlers leaves it empty.
def without_guardian:
  map(
    if type == "object" and ((.hooks? | type) == "array") and any(.hooks[]; is_guardian) then
      (.hooks |= map(select(is_guardian | not)))
      | if (.hooks | length) == 0 then empty else . end
    else . end
  );

(.[0] // {}) as $base
| .[1] as $g
| if ($base | type) != "object" then
    error("the settings file is not a JSON object")
  elif ($base.hooks != null and ($base.hooks | type) != "object") then
    error("its \"hooks\" value is not an object")
  else
    $base
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
