#!/usr/bin/env python3
"""visualize_all.py — Run parse_trace.py -v on all trace_* files in the current
directory, several at a time.

    ./visualize_all.py               render stale SVGs once, in parallel
    ./visualize_all.py --force       re-render every SVG
    ./visualize_all.py --watch       keep rendering as traces change (used by
                                     auto_visualize_all.bash)
    -j N                             max concurrent renders (default: 8)

Each trace renders in its own process. In --watch mode a slow render (a live
OpenCode trace can spend tens of seconds in `opencode export`) only occupies
its own slot: other traces keep being scheduled, and nothing waits for it.
"""

import argparse
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "parse_trace.py"
if not SCRIPT.exists():
    print(f"ERROR: {SCRIPT} not found", file=sys.stderr)
    sys.exit(1)

# A live OpenCode trace does not grow while a sub-agent runs (sub-agents are
# opaque in the parent's JSONL), but re-parsing pulls the child sessions via
# live-export. parse_trace.py marks such SVGs with the in-flight count; re-render
# them on a timer while the trace was touched recently (a killed run keeps its
# in-flight marker forever, hence the age cap).
LIVE_REFRESH_S = 10
LIVE_MAX_AGE_S = 24 * 3600
_LIVE_RE = re.compile(rb"<!-- harvest-live: in_flight_subagents=(\d+) -->")

RENDER_TIMEOUT_S = 300
POLL_S = 0.5


def live_subagents(svg: Path) -> int:
    try:
        with open(svg, "rb") as fh:
            m = _LIVE_RE.search(fh.read(256))
    except OSError:
        return 0
    return int(m.group(1)) if m else 0


def list_traces() -> list[Path]:
    out = []
    for f in sorted(Path().glob("trace_*")):
        if f.suffix in (".svg", ".json"):
            continue  # already-generated outputs
        # Skip derived files (_readable.txt, _file_io.json, etc.)
        if "_readable" in f.stem or "_file_io" in f.stem:
            continue
        out.append(f)
    return out


def svg_for(f: Path) -> Path:
    return f.parent / (f.stem + "_timeline.svg")


def needs_render(f: Path, launched_mtime: dict[Path, float]) -> bool:
    try:
        mtime = f.stat().st_mtime
    except OSError:
        return False  # trace vanished
    # A trace that grew while it was being rendered is stale even though the
    # SVG was written after the trace's new mtime.
    if f in launched_mtime and mtime > launched_mtime[f]:
        return True
    svg = svg_for(f)
    if not svg.exists() or svg.stat().st_mtime < mtime:
        return True
    now = time.time()
    return (
        now - mtime < LIVE_MAX_AGE_S
        and now - svg.stat().st_mtime >= LIVE_REFRESH_S
        and live_subagents(svg) > 0
    )


class Job:
    def __init__(self, f: Path):
        self.f = f
        self.started = time.time()
        self.stderr = tempfile.TemporaryFile()
        self.proc = subprocess.Popen(
            [sys.executable, str(SCRIPT), str(f), "-v"],
            stdout=subprocess.DEVNULL, stderr=self.stderr,
        )

    def finish(self) -> None:
        """Report the result of a process that has exited."""
        elapsed = time.time() - self.started
        if self.proc.returncode == 0:
            print(f"  {self.f.name} -> {svg_for(self.f).name}  ({elapsed:.1f}s)", flush=True)
        else:
            print(f"  {self.f.name} FAILED ({elapsed:.1f}s)", flush=True)
            self.stderr.seek(0)
            err = self.stderr.read().decode(errors="replace").strip()
            if err:
                print(f"    {err[-300:]}", file=sys.stderr, flush=True)
        self.stderr.close()


def run(jobs: int, force: bool, watch: bool, interval: float) -> int:
    running: dict[Path, Job] = {}
    launched_mtime: dict[Path, float] = {}
    failures = 0
    first_pass = True
    next_scan = 0.0
    queue: list[Path] = []

    while True:
        now = time.time()
        if now >= next_scan:
            pending = set(queue)
            for f in list_traces():
                if f in running or f in pending:
                    continue
                if (force and first_pass) or needs_render(f, launched_mtime):
                    queue.append(f)
            # Most recently modified first: live traces get a slot before the
            # backlog of old ones.
            queue.sort(key=lambda p: -p.stat().st_mtime if p.exists() else 0)
            first_pass = False
            next_scan = now + interval

        while queue and len(running) < jobs:
            f = queue.pop(0)
            if not f.exists():
                continue
            launched_mtime[f] = f.stat().st_mtime
            running[f] = Job(f)

        for f, job in list(running.items()):
            rc = job.proc.poll()
            if rc is None and time.time() - job.started > RENDER_TIMEOUT_S:
                job.proc.kill()
                rc = job.proc.wait()
            if rc is not None:
                job.finish()
                failures += rc != 0
                del running[f]

        if not watch and not queue and not running:
            return 1 if failures else 0
        time.sleep(POLL_S)


def main() -> None:
    ap = argparse.ArgumentParser(description="Render trace_* timelines in parallel.")
    ap.add_argument("-j", "--jobs", type=int, default=min(8, os.cpu_count() or 1),
                    help="max concurrent renders (default: %(default)s)")
    ap.add_argument("--force", action="store_true",
                    help="re-render every SVG, not only stale ones")
    ap.add_argument("--watch", action="store_true",
                    help="keep running and re-render traces as they change")
    ap.add_argument("--interval", type=float, default=10.0,
                    help="seconds between directory scans in --watch mode")
    args = ap.parse_args()
    if not list_traces():
        print("No trace_* files found in current directory.")
        return
    try:
        sys.exit(run(max(1, args.jobs), args.force, args.watch, args.interval))
    except KeyboardInterrupt:
        sys.exit(130)


if __name__ == "__main__":
    main()
