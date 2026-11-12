#!/usr/bin/env python3
"""
capsules.py — partition a C project into capsules for C -> Rust translation.

Builds the call graph by running callgraph.py (its `dot` mode), groups the
functions into capsules by CONNECTED COMPONENT (natural clusters that do not
call across the boundary), orders the capsules dependency-first, and writes
ONE combined Markdown plan describing every capsule for a sub-agent to pick up.

A capsule is a cluster of functions translated to Rust together, as one unit,
flipped from C to Rust all at once. See translation-plan-v2.md for the model.

Usage:
    python3 capsules.py <compile_commands.json> [--callgraph PATH] [-o PLAN.md]

    --callgraph PATH   path to callgraph.py (default: ./callgraph.py)
    -o PATH            write the plan here (default: stdout)

Requires: python3, and whatever callgraph.py needs (clang, llvm-link, opt,
llvm-dis). This script adds no new dependency of its own.
"""

import re
import sys
import shutil
import argparse
import subprocess
from pathlib import Path
from collections import defaultdict


# ---------------------------------------------------------------------------
# Step 1: get the call graph from callgraph.py's `dot` output
# ---------------------------------------------------------------------------

def run_callgraph_dot(callgraph_py: Path, compile_commands: Path) -> str:
    """Run `callgraph.py dot <cc.json>` and return its stdout (DOT text)."""
    cmd = [sys.executable, str(callgraph_py), "dot", str(compile_commands)]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        sys.exit(
            f"callgraph.py failed (exit {proc.returncode}).\n"
            f"command: {' '.join(cmd)}\n{proc.stderr}"
        )
    # Surface callgraph.py's own warnings (skipped files, missing llvm-dis) so
    # the caller knows the graph may be incomplete.
    for line in proc.stderr.splitlines():
        if "WARN" in line or "SKIP" in line:
            print(f"[callgraph] {line.strip()}", file=sys.stderr)
    return proc.stdout


# ---------------------------------------------------------------------------
# Step 2: parse the DOT into edges + the indirect-call flag set
# ---------------------------------------------------------------------------

_EDGE_RE = re.compile(r'^\s*"(?P<src>(?:[^"\\]|\\.)*)"\s*->\s*"(?P<dst>(?:[^"\\]|\\.)*)"')
_INDIRECT_RE = re.compile(r'^\s*//\s*indirect-call-site:\s*(?P<name>.+?)\s*$')
_NODE_RE = re.compile(r'^\s*"(?P<name>(?:[^"\\]|\\.)*)"\s*\[')


def parse_dot(dot: str):
    """
    Return (nodes, edges, indirect):
      nodes    - set of every function name seen
      edges    - list of (caller, callee) directed pairs
      indirect - set of functions flagged as having a function-pointer call
    """
    nodes: set[str] = set()
    edges: list[tuple[str, str]] = []
    indirect: set[str] = set()

    for line in dot.splitlines():
        m = _INDIRECT_RE.match(line)
        if m:
            indirect.add(_unescape(m.group("name")))
            continue
        m = _EDGE_RE.match(line)
        if m:
            src = _unescape(m.group("src"))
            dst = _unescape(m.group("dst"))
            nodes.add(src)
            nodes.add(dst)
            edges.append((src, dst))
            continue
        m = _NODE_RE.match(line)
        if m:
            nodes.add(_unescape(m.group("name")))
    return nodes, edges, indirect


def _unescape(s: str) -> str:
    return s.replace('\\"', '"')


# ---------------------------------------------------------------------------
# Step 3: group into capsules by CONNECTED COMPONENT
# ---------------------------------------------------------------------------

def connected_components(nodes: set[str], edges: list[tuple[str, str]]) -> list[set[str]]:
    """
    Weakly-connected components: treat call edges as undirected and find the
    clusters of functions that are reachable from one another. Each cluster is
    one capsule — nothing outside it calls into it except across the component
    boundary, which is exactly where the capsule's trampolines will sit.

    This is the loosest grouping: it refuses to cut anywhere a call path
    exists, so on a real library (where almost everything is reachable through
    shared helpers) it tends to produce one giant capsule. Use `agglomerative`
    with --max-size / --max-cross when that happens.
    """
    # Union-Find over all node names.
    parent: dict[str, str] = {n: n for n in nodes}

    def find(x: str) -> str:
        root = x
        while parent[root] != root:
            root = parent[root]
        # path compression
        while parent[x] != root:
            parent[x], x = root, parent[x]
        return root

    def union(a: str, b: str) -> None:
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[ra] = rb

    for a, b in edges:
        union(a, b)

    groups: dict[str, set[str]] = defaultdict(set)
    for n in nodes:
        groups[find(n)].add(n)
    return list(groups.values())


def _edge_weights(edges: list[tuple[str, str]]) -> dict[tuple[str, str], int]:
    """Count call edges between each unordered pair of functions (self-calls
    dropped). Two functions that call each other a lot are strong merge
    candidates, so we weight by the number of calls in either direction."""
    w: dict[tuple[str, str], int] = defaultdict(int)
    for a, b in edges:
        if a == b:
            continue
        key = (a, b) if a <= b else (b, a)
        w[key] += 1
    return dict(w)


def agglomerative(
    nodes: set[str],
    edges: list[tuple[str, str]],
    max_cross: float,
) -> list[set[str]]:
    """
    Build capsules bottom-up by greedy MODULARITY-gain merging — the standard,
    well-behaved graph-clustering move — controlled by one knob:

      max_cross  — target ceiling on the FRACTION of all calls that cross a
                   capsule boundary (0.0-1.0). This is the "how many calls may
                   happen between capsules" tunable. The process keeps merging
                   while the partition's overall cross-ratio is still above
                   this target, then stops. So LOWER max_cross => keep merging
                   longer => fewer, larger, more self-contained capsules;
                   HIGHER => stop sooner => more, smaller capsules with more
                   crossings between them.

    Why modularity rather than raw edge count: modularity gain measures whether
    two groups are more connected to each other than chance would predict, so
    it merges genuinely cohesive clusters and leaves weakly-linked ones apart,
    instead of greedily chaining everything through shared helpers (which is
    what produced the single blob). Deterministic: ties broken by member names.
    """
    fweight = _edge_weights(edges)
    m = sum(fweight.values())  # total undirected call-edge weight
    if m == 0:
        return [{n} for n in sorted(nodes)]

    caps: dict[int, set[str]] = {i: {n} for i, n in enumerate(sorted(nodes))}
    member_cap: dict[str, int] = {n: i for i, cap in caps.items() for n in cap}

    # Weighted degree of each function (its share of all call endpoints).
    fdeg: dict[str, int] = defaultdict(int)
    for (a, b), wt in fweight.items():
        fdeg[a] += wt
        fdeg[b] += wt

    # Per-capsule aggregates kept incrementally: total degree, and weight of
    # edges to each neighbouring capsule.
    cap_deg: dict[int, int] = {i: fdeg[next(iter(cap))] for i, cap in caps.items()}
    cap_adj: dict[int, dict[int, int]] = {i: defaultdict(int) for i in caps}
    for (a, b), wt in fweight.items():
        ci, cj = member_cap[a], member_cap[b]
        if ci != cj:
            cap_adj[ci][cj] += wt
            cap_adj[cj][ci] += wt

    def current_cross_ratio() -> float:
        cross = sum(w for i in cap_adj for w in cap_adj[i].values()) / 2
        return cross / m

    def modularity_gain(ci: int, cj: int) -> float:
        # dQ for merging ci and cj = e_ij/m - 2 * (deg_i/2m)(deg_j/2m)
        e_ij = cap_adj[ci].get(cj, 0)
        return e_ij / m - 2 * (cap_deg[ci] / (2 * m)) * (cap_deg[cj] / (2 * m))

    def do_merge(ci: int, cj: int) -> None:
        # Fold cj into ci and update aggregates.
        caps[ci] |= caps[cj]
        for n in caps[cj]:
            member_cap[n] = ci
        cap_deg[ci] += cap_deg[cj]
        for nb, w in list(cap_adj[cj].items()):
            if nb == ci:
                continue
            cap_adj[ci][nb] += w
            cap_adj[nb][ci] += w
            del cap_adj[nb][cj]
        cap_adj[ci].pop(cj, None)
        del cap_adj[cj]
        del caps[cj]
        del cap_deg[cj]

    # Merge until no beneficial merge remains, bounded by the cross-ratio
    # target. We keep merging the best (highest modularity gain) pair; once the
    # partition's cross-ratio has dropped to the target, we only continue if a
    # merge still strictly improves modularity, so a high max_cross stops early
    # and a low one keeps consolidating.
    while True:
        best = None  # (gain, ci, cj)
        for ci in list(cap_adj):
            for cj, _w in cap_adj[ci].items():
                if cj <= ci:
                    continue
                gain = modularity_gain(ci, cj)
                tie = (min(caps[ci]), min(caps[cj]))
                cand = (gain, tie, ci, cj)
                if best is None or cand[:2] > best[:2]:
                    best = cand
        if best is None:
            break
        gain, _tie, ci, cj = best
        above_target = current_cross_ratio() > max_cross
        # Merge if it helps modularity, OR if we're still over the cross-ratio
        # target and the merge doesn't actively hurt structure.
        if gain > 0 or (above_target and gain >= 0):
            do_merge(ci, cj)
            continue
        if above_target and gain < 0:
            # Over target but every remaining merge worsens modularity: take the
            # least-harmful one to keep driving the cross-ratio down.
            do_merge(ci, cj)
            continue
        break

    return list(caps.values())


# ---------------------------------------------------------------------------
# Metric: interconnectedness vs intraconnectedness
# ---------------------------------------------------------------------------

def score_partition(
    capsules: list[set[str]], edges: list[tuple[str, str]]
) -> dict:
    """
    Measure how well the partition separates the call graph.

      internal      - calls whose caller and callee share a capsule
      cross         - calls that cross a capsule boundary
      cohesion      - internal / total   (want HIGH: calls kept inside)
      cross_ratio   - cross / total      (want LOW: the complement)
      modularity    - standard graph-clustering quality in [-0.5, 1];
                      higher means the partition captures real structure
                      rather than cutting arbitrarily.

    Also returns per-capsule cohesion so you can spot a weak capsule.
    """
    member_cap: dict[str, int] = {}
    for i, cap in enumerate(capsules):
        for n in cap:
            member_cap[n] = i

    internal = cross = 0
    # per-capsule tallies for cohesion and modularity
    in_edges = defaultdict(int)       # calls inside capsule i
    deg = defaultdict(int)            # total edge-endpoints in capsule i
    touch = defaultdict(lambda: [0, 0])  # [internal, boundary] per capsule
    m = 0
    for a, b in edges:
        if a == b:
            continue
        m += 1
        ci, cj = member_cap.get(a), member_cap.get(b)
        if ci is None or cj is None:
            continue
        deg[ci] += 1
        deg[cj] += 1
        if ci == cj:
            internal += 1
            in_edges[ci] += 1
            touch[ci][0] += 1
        else:
            cross += 1
            touch[ci][1] += 1
            touch[cj][1] += 1

    total = internal + cross
    cohesion = (internal / total) if total else 1.0
    cross_ratio = (cross / total) if total else 0.0

    # Newman modularity for a directed graph approximated on the undirected
    # projection: Q = sum_i ( in_i/m - (deg_i/2m)^2 ).
    modularity = 0.0
    if m:
        for i in range(len(capsules)):
            modularity += (in_edges[i] / m) - (deg[i] / (2 * m)) ** 2

    per_capsule = []
    for i, cap in enumerate(capsules):
        ii, bb = touch[i]
        t = ii + bb
        per_capsule.append({
            "size": len(cap),
            "internal_calls": ii,
            "boundary_calls": bb,
            "cohesion": (ii / t) if t else 1.0,
        })

    return {
        "capsules": len(capsules),
        "functions": sum(len(c) for c in capsules),
        "total_calls": total,
        "internal_calls": internal,
        "cross_calls": cross,
        "cohesion": cohesion,
        "cross_ratio": cross_ratio,
        "modularity": modularity,
        "per_capsule": per_capsule,
    }


# ---------------------------------------------------------------------------
# Step 4: order capsules dependency-first (topological over inter-capsule edges)
# ---------------------------------------------------------------------------

def order_capsules(
    capsules: list[set[str]], edges: list[tuple[str, str]]
) -> tuple[list[int], dict[int, set[int]]]:
    """
    Order capsules so that a capsule is translated AFTER everything it calls
    into. Returns (ordering, deps) where deps[i] is the set of capsule indices
    that capsule i depends on (calls into).

    For connected components there are no edges BETWEEN components by
    construction, so the inter-capsule graph is empty and any order is valid.
    We still compute it generically: if a later merge step ever joins
    components, this keeps working. Ordering falls back to capsule size
    (smallest first — leaf-like utility clusters before the big ones) so the
    output is deterministic and roughly dependency-shaped.
    """
    index_of: dict[str, int] = {}
    for i, members in enumerate(capsules):
        for fn in members:
            index_of[fn] = i

    deps: dict[int, set[int]] = {i: set() for i in range(len(capsules))}
    for caller, callee in edges:
        ci, cj = index_of.get(caller), index_of.get(callee)
        if ci is not None and cj is not None and ci != cj:
            deps[ci].add(cj)  # capsule ci depends on capsule cj

    # Topological order (dependencies first), ties broken by size then name.
    def sort_key(i: int) -> tuple:
        members = sorted(capsules[i])
        return (len(members), members[0] if members else "")

    ordered: list[int] = []
    placed: set[int] = set()
    remaining = sorted(range(len(capsules)), key=sort_key)
    # Kahn-ish: repeatedly take capsules whose deps are all already placed.
    progress = True
    while remaining and progress:
        progress = False
        still: list[int] = []
        for i in remaining:
            if deps[i] <= placed:
                ordered.append(i)
                placed.add(i)
                progress = True
            else:
                still.append(i)
        remaining = still
    # Any left are in a dependency cycle between capsules; append by size.
    ordered.extend(remaining)
    return ordered, deps


# ---------------------------------------------------------------------------
# Step 5: compute each capsule's external boundary (its would-be trampolines)
# ---------------------------------------------------------------------------

def external_boundary(
    members: set[str], all_edges: list[tuple[str, str]]
) -> set[str]:
    """
    The functions in this capsule that are called from OUTSIDE it. Each such
    name is a symbol other code already calls by that exact name, so it becomes
    a required `extern "C"` trampoline when the capsule flips to Rust.
    """
    boundary: set[str] = set()
    for caller, callee in all_edges:
        if callee in members and caller not in members:
            boundary.add(callee)
    return boundary


# ---------------------------------------------------------------------------
# Step 6: write the combined Markdown plan
# ---------------------------------------------------------------------------

def write_plan(
    capsules: list[set[str]],
    ordering: list[int],
    deps: dict[int, set[int]],
    edges: list[tuple[str, str]],
    indirect: set[str],
    compile_commands: Path,
    metric: dict,
    grouping_desc: str,
) -> str:
    out: list[str] = []
    w = out.append

    # A stable capsule label: C<position in translation order>.
    label_of: dict[int, str] = {idx: f"C{pos + 1}" for pos, idx in enumerate(ordering)}

    w("# Capsule translation plan")
    w("")
    w(f"Source: `{compile_commands}`")
    w(f"Grouping: {grouping_desc}")
    w(f"Capsules: {len(capsules)} · Functions: {sum(len(c) for c in capsules)} · "
      f"Flagged (function-pointer) functions: {len(indirect)}")
    w("")
    # Scorecard: how well the partition separates the call graph.
    w("## Partition quality")
    w("")
    w(f"- **Cohesion (intra):** {metric['cohesion']:.1%} of calls stay inside a "
      f"capsule — higher is better.")
    w(f"- **Cross-ratio (inter):** {metric['cross_ratio']:.1%} of calls cross a "
      f"capsule boundary — lower is better; each crossing is a trampoline.")
    w(f"- **Modularity:** {metric['modularity']:.3f} "
      f"(−0.5…1; higher means the split follows real structure, not arbitrary cuts).")
    w(f"- **Calls:** {metric['internal_calls']} internal / "
      f"{metric['cross_calls']} crossing / {metric['total_calls']} total.")
    w("")
    w("Capsules are listed in translation order (dependencies first). Each flips "
      "from C to Rust as one unit. A function under **External boundary** is "
      "called from another capsule, so it needs an `extern \"C\"` trampoline "
      "with its exact name. A function marked **[indirect]** contains a call "
      "through a function pointer whose target the static graph could not name "
      "— resolve it from the source before trusting this capsule's boundary.")
    w("")
    w("---")
    w("")

    for pos, idx in enumerate(ordering):
        members = capsules[idx]
        label = label_of[idx]
        dep_labels = sorted(label_of[d] for d in deps[idx])
        boundary = external_boundary(members, edges)
        flagged = sorted(m for m in members if m in indirect)

        pc = metric["per_capsule"][idx]
        w(f"## {label} — {len(members)} function(s)")
        w("")
        w(f"- **Cohesion:** {pc['cohesion']:.0%} "
          f"({pc['internal_calls']} internal / {pc['boundary_calls']} boundary calls)")
        w(f"- **Depends on:** {', '.join(dep_labels) if dep_labels else 'none (translate first)'}")
        if boundary:
            w(f"- **External boundary (trampolines):** "
              + ", ".join(f"`{b}`" for b in sorted(boundary)))
        else:
            w("- **External boundary (trampolines):** none "
              "(nothing outside this capsule calls into it)")
        if flagged:
            w(f"- **Unresolved function pointers:** "
              + ", ".join(f"`{f}`" for f in flagged)
              + " — recover their targets from the source.")
        else:
            w("- **Unresolved function pointers:** none")
        w("")
        w("<details><summary>Member functions</summary>")
        w("")
        for m in sorted(members):
            tag = "  **[indirect]**" if m in indirect else ""
            w(f"- `{m}`{tag}")
        w("")
        w("</details>")
        w("")

    return "\n".join(out) + "\n"


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> None:
    ap = argparse.ArgumentParser(
        description="Partition a C project into capsules for C->Rust translation.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("compile_commands", type=Path, help="path to compile_commands.json")
    ap.add_argument("--callgraph", type=Path, default=Path("callgraph.py"),
                    help="path to callgraph.py (default: ./callgraph.py)")
    ap.add_argument("-o", "--output", type=Path, default=None,
                    help="write the plan here (default: stdout)")
    ap.add_argument("--group", choices=("agglomerative", "components"),
                    default="agglomerative",
                    help="grouping method. 'agglomerative' (default) uses "
                         "--max-cross; 'components' is the loose "
                         "connected-component grouping (one capsule per island).")
    ap.add_argument("--max-cross", type=float, default=0.5, metavar="F",
                    help="max fraction of all calls that may cross a capsule "
                         "boundary, 0.0-1.0 (default 0.5). Lower = fewer, larger, "
                         "more self-contained capsules; higher = more, smaller "
                         "ones. This is the 'calls allowed between capsules' knob.")
    args = ap.parse_args()

    if not args.compile_commands.exists():
        sys.exit(f"no such file: {args.compile_commands}")
    if not args.callgraph.exists():
        sys.exit(f"callgraph.py not found at {args.callgraph} (pass --callgraph PATH)")
    if not (0.0 <= args.max_cross <= 1.0):
        sys.exit("--max-cross must be between 0.0 and 1.0")

    dot = run_callgraph_dot(args.callgraph, args.compile_commands)
    nodes, edges, indirect = parse_dot(dot)
    if not nodes:
        sys.exit("callgraph.py produced no functions — check its warnings above.")

    if args.group == "components":
        capsules = connected_components(nodes, edges)
        grouping_desc = "connected components (loose; one capsule per island)"
    else:
        capsules = agglomerative(nodes, edges, args.max_cross)
        grouping_desc = f"agglomerative (max-cross={args.max_cross:.2f})"

    ordering, deps = order_capsules(capsules, edges)
    metric = score_partition(capsules, edges)
    plan = write_plan(capsules, ordering, deps, edges, indirect,
                      args.compile_commands, metric, grouping_desc)

    if args.output:
        args.output.write_text(plan)
        print(f"wrote {args.output} ({len(capsules)} capsules, {len(nodes)} "
              f"functions, cohesion {metric['cohesion']:.1%}, "
              f"modularity {metric['modularity']:.3f})", file=sys.stderr)
    else:
        sys.stdout.write(plan)


if __name__ == "__main__":
    main()