# Quality runner task

You run one scripted step of the quality work and report its result. The script does the work;
you start it, wait for it, and report. The main agent decides from your report and from the files
that the script writes, so copy numbers from the script output only.

Your task message gives your checkout (run every command from its root) and one `qflow.py`
command: `baseline` (in the main checkout) or `integrate` (in the integration worktree of a local
wave). `S` below means `inputs/agent_skills/rust_quality/scripts`.

{RULES}

## Steps

1. The command runs long (baseline: 10 to 40 minutes; integrate: about 5 minutes per branch). Run
   it in the background, as `SKILL.md` ("Long commands") shows:

   ```
   nohup sh -c 'python3 S/qflow.py <arguments> > scratch/qflow.log 2>&1; echo "EXIT $?" >> scratch/qflow.log' > /dev/null 2>&1 &
   ```

   Then wait for the `EXIT` line, and read `tail -40 scratch/qflow.log`.
2. `EXIT 0`: report.
3. Another exit code: read the log.
   - `baseline`, and the crate or the test harness does not build before any change: fix only the
     build error (in the crate, or the C source path and the compile flags in
     `verify_env/CMakeLists.txt`; never a test). Commit the fix as `quality: fix the baseline
     build`, remove `shared/regress/baseline` and `shared/quality`, and run the command again.
     At most 2 attempts.
   - Any other failure: do not fix code. Report the failure and the last 20 lines of the log.

You do not edit code for any other reason. Do not merge, do not remove worktrees.

## Reply

At most 15 lines: the exit code, the summary that the script printed at its end (copy it), and any
build fix that you made.
