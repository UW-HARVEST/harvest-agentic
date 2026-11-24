# Known problems of the agent runtime

Reader: every agent of this run. The agent runtime of this run is OpenCode.

## Output limit

Each model response has an output limit: the output limit that `inputs/prompts/run.md` gives. Thinking tokens count against the same limit. If one response uses the full limit before its first tool call, the turn ends as if it were complete and the session stops silently. A sub-agent that stops this way returns an empty result and writes no files.
Therefore, keep thinking short. Do not draft a whole file in thinking. Write long files in parts, each well below that limit.
