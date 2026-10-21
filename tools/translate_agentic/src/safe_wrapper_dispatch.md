5. In the same message as the scouts, also send one Safe API Wrapper Designer. It designs the safe
   Rust boundary of the library. Give it this prompt:

   ```
   Read inputs/prompts/SAFE_WRAPPER_DESIGNER.md and do the task in it.
   Context window: <sub-agent window from 0.1>. Output token limit: <sub-agent output limit from 0.1>.
   ```

   It writes `scout/safe_wrapper/DESIGN.md` and a code skeleton in `scout/safe_wrapper/skeleton/`,
   and replies with a boundary summary. The Invariants section "Safe API wrappers" tells how the
   plan uses the design.
