#!/usr/bin/env python3
"""visualize.py — Run parse_trace.py -v on all trace_* files in current directory."""

import re
import subprocess
import sys
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


def live_subagents(svg: Path) -> int:
    try:
        with open(svg, "rb") as fh:
            m = _LIVE_RE.search(fh.read(256))
    except OSError:
        return 0
    return int(m.group(1)) if m else 0


traces = sorted(Path().glob("trace_*"))
if not traces:
    print("No trace_* files found in current directory.")
    sys.exit(0)

for f in traces:
    if f.suffix == ".svg":
        continue  # skip already-generated SVGs
    if f.suffix == ".json":
        continue  # skip already-generated JSONs
    # Skip derived files (_readable.txt, _file_io.json, etc.)
    if "_readable" in f.stem or "_file_io" in f.stem:
        continue
    svg = f.parent / (f.stem + "_timeline.svg")
    if svg.exists() and svg.stat().st_mtime >= f.stat().st_mtime:
        now = time.time()
        live = (
            now - f.stat().st_mtime < LIVE_MAX_AGE_S
            and now - svg.stat().st_mtime >= LIVE_REFRESH_S
            and live_subagents(svg) > 0
        )
        if not live:
            # print(f"  {f.name} -> {svg.name}  (up to date, skip)")
            continue
    print(f"  {f.name} ... ", end="", flush=True)
    r = subprocess.run(
        ["python3", str(SCRIPT), str(f), "-v"],
        capture_output=True, text=True, timeout=300,
    )
    if r.returncode == 0:
        out = f.parent / (f.stem + "_timeline.svg")
        print(out.name)
    else:
        print(f"FAILED")
        if r.stderr:
            print(f"    {r.stderr.strip()[:120]}", file=sys.stderr)
