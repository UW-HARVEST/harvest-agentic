# Static analysis tool wishlist

Reader: the main agent of the stage, when `inputs/prompts/run.md` lists this file.

As you work, pay attention to moments where you think:
- "If I had a tool that could tell me X, I could skip this lengthy file reading / reasoning."
- "If I had a tool that could do Y, I would have much higher confidence in this step."

Whenever such a thought arises, **immediately** append one JSON object (on a single line) to
the file `tool_wishlist.json` in the working directory. Do not wait until the end — record the wish as soon as it occurs,
while the context is fresh. Multiple entries are encouraged; record every distinct need.

Each entry must be a single-line JSON object with exactly these fields:

```
{"category": "...", "description": "...", "language": "...", "soundness": "...", "completeness": "...", "value": 0}
```

Field definitions:
- `category`: `"info_query"` (read-only analysis that answers a question) or `"code_edit"` (a transformation/rewrite tool)
- `description`: plain English description of what the tool does — **no implementation details**, just what it gives you and why it would help
- `language`: `"C"`, `"Rust"`, `"C_and_Rust"`, or another language name
- `soundness`: `"required"` (must never give wrong answers), `"preferred"`, or `"not_needed"` (approximate/heuristic output is fine)
- `completeness`: `"required"` (must cover all cases), `"preferred"`, or `"not_needed"` (partial results are useful enough)
- `value`: integer 0–10 estimating how much this tool would have helped you in this specific task
