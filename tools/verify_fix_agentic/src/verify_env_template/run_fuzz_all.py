#!/usr/bin/env python3
"""Run several fuzz campaigns at the same time, each through run_fuzz.sh.

Usage, from the project directory, after build_fuzz.sh:

  RUST_LIB_PATH=<abs path to the Rust .so> ./verify_env/run_fuzz_all.py [options] [PROP[=SECONDS] ...]

PROP is Suite.Property. With no PROP, the script runs every property that the fuzzing binary
lists. PROP=SECONDS gives that property its own duration.

Options:
  --seconds S   duration of each campaign without its own (default: FUZZ_DURATION_SECONDS,
                else the run_fuzz.sh default)
  --jobs N      run up to N campaigns at once, without the resource checks below
                (environment: FUZZ_JOBS=N)
  --list        print the properties of the fuzzing binary and stop

Without --jobs, the script chooses the number of campaigns from the resources, and checks them
again every 2 seconds while it runs:
  - at most (CPUs that this command can use - 1) campaigns, and at least 1. The CPUs come from
    the CPU affinity and the CPU quota of the cgroups above this command;
  - a new campaign starts only when at least 1 CPU of the machine is idle;
  - a new campaign starts only when the free memory is enough for one more campaign, for the
    growth of the running campaigns up to that size, and for a reserve of 512 MiB. The free
    memory is the smallest of: the available memory of the machine, and the free part of each
    memory-limited cgroup above this command (cgroup v2 or v1). One campaign is expected to use
    as much as the largest campaign so far, and at least 512 MiB;
  - at most one new campaign starts at each check, so that the next check sees its load;
  - when no campaign runs, the next one starts in any case, so the batch always ends.
The script writes each decision to wait into its output. It does not stop a running campaign.
A campaign grows while it runs, so early in a batch the estimate can be too low. When campaigns
end with "[harvest] killed" or an out-of-memory report, or when the machine has resources that
the script does not see, set the number yourself with --jobs.

Each campaign keeps the limits of run_fuzz.sh: its own cgroup memory limit, the RSS limit, the
duration and the per-input time limit. The FUZZ_* settings of run_fuzz.sh apply to every
campaign. Each run writes a new directory verify_env/fuzz-artifacts/batch-<time>-<id>/ with one
log for each campaign and summary.txt. The summary has one line for each campaign (exit status,
seconds, peak memory of its processes, edges, corpus size, runs per second, artifacts
directory). The last line of the output is "ALL DONE: <passed> passed, <failed> failed". The
exit status is 0 only when every campaign passed. A TERM, INT or HUP signal stops all running
campaigns.
"""

import argparse
import os
import re
import signal
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
BINARY = os.path.join(HERE, "build-fuzz", "verification_tests")
RUNNER = os.path.join(HERE, "run_fuzz.sh")
CGROUP_ROOT = "/sys/fs/cgroup"
TICK_SECONDS = 2.0
MIB = 1 << 20
# The smallest memory that the script expects of one campaign, also before it has measured one.
MIN_CAMPAIGN_BYTES = 512 * MIB
# Free memory that the script leaves for the agent, its builds and other runs.
RESERVE_BYTES = 512 * MIB
# cgroup v1 writes a value near 2^63 for "no limit".
V1_NO_LIMIT = 1 << 60
PROP_RE = re.compile(r"^[A-Za-z0-9_]+\.[A-Za-z0-9_]+$")


def log(message):
    print(time.strftime("[%H:%M:%S] ") + message, flush=True)


def read(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return None


def cgroup_dirs(controller):
    """(version, directories) of the cgroups of this process that can limit `controller`
    ("memory" or "cpu"): from its own cgroup up to the root of the hierarchy. cgroup v1 is used
    when it has the controller, else cgroup v2."""
    v2 = None
    for line in (read("/proc/self/cgroup") or "").splitlines():
        hierarchy, _, rest = line.partition(":")
        controllers, _, path = rest.partition(":")
        if controller in controllers.split(","):
            for name in (controllers, controller):
                root = os.path.join(CGROUP_ROOT, name)
                if os.path.isdir(root):
                    return 1, _up_to(root, path)
        elif hierarchy == "0" and controllers == "":
            v2 = path
    if v2 is None:
        return 2, []
    return 2, _up_to(CGROUP_ROOT, v2)


def _up_to(root, path):
    dirs, current = [], os.path.normpath(root + "/" + path.strip("/"))
    while True:
        dirs.append(current)
        if current == root or not current.startswith(root):
            return dirs
        current = os.path.dirname(current)


def cpu_count():
    """CPUs that this command can use: its CPU affinity and the CPU quotas of its cgroups."""
    try:
        cpus = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        cpus = os.cpu_count() or 1
    version, dirs = cgroup_dirs("cpu")
    for cg in dirs:
        if version == 2:
            quota, _, period = (read(cg + "/cpu.max") or "max").partition(" ")
        else:
            quota, period = read(cg + "/cpu.cfs_quota_us"), read(cg + "/cpu.cfs_period_us")
        if quota and period and quota not in ("max", "-1"):
            cpus = min(cpus, max(1, int(quota) // int(period)))
    return max(1, cpus)


def _stat_value(text, key):
    for line in (text or "").splitlines():
        name, _, value = line.partition(" ")
        if name == key:
            return int(value)
    return 0


def free_memory():
    """(bytes, where): the smallest free memory of the machine and of the limited cgroups."""
    best, where = None, "the machine"
    for line in (read("/proc/meminfo") or "").splitlines():
        if line.startswith("MemAvailable:"):
            best = int(line.split()[1]) * 1024
    version, dirs = cgroup_dirs("memory")
    names = (("memory.max", "memory.current", "inactive_file") if version == 2 else
             ("memory.limit_in_bytes", "memory.usage_in_bytes", "total_inactive_file"))
    for cg in dirs:
        limit, current = read(cg + "/" + names[0]), read(cg + "/" + names[1])
        if not limit or not limit.isdigit() or not current or int(limit) >= V1_NO_LIMIT:
            continue
        # Inactive file cache can be reclaimed, so it is not counted as used.
        used = int(current) - _stat_value(read(cg + "/memory.stat"), names[2])
        free = max(0, int(limit) - used)
        if best is None or free < best:
            best, where = free, "cgroup " + os.path.basename(cg)
    return (best if best is not None else 1 << 62), where


def wait_reason(running, idle_cpus, free, where, expected, growth):
    """(kind, text) of the reason not to start one more campaign now, or None."""
    if running == 0:
        return None
    if idle_cpus < 1.0:
        return "cpu", "fewer than 1 idle CPU (%.1f)" % idle_cpus
    need = expected + growth + RESERVE_BYTES
    if free < need:
        return "memory", ("free memory %d MiB (%s) is below %d MiB: one more campaign, the "
                          "growth of the running ones, and a reserve of %d MiB"
                          % (free // MIB, where, need // MIB, RESERVE_BYTES // MIB))
    return None


class CpuMeter:
    """Idle CPUs of the machine between two calls, from /proc/stat."""

    def __init__(self):
        self.last = self._sample()

    @staticmethod
    def _sample():
        fields = (read("/proc/stat") or "cpu 0 0 0 0").splitlines()[0].split()[1:]
        values = [int(v) for v in fields]
        idle = values[3] + (values[4] if len(values) > 4 else 0)
        return idle, sum(values)

    def idle_cpus(self, machine_cpus):
        now = self._sample()
        idle = now[0] - self.last[0]
        total = now[1] - self.last[1]
        self.last = now
        if total <= 0:
            return float(machine_cpus)
        return machine_cpus * idle / total


def process_table():
    """(children by parent PID, resident memory by PID) of all processes, from /proc."""
    children, rss = {}, {}
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        status = read("/proc/%s/status" % name)
        if not status:
            continue
        ppid, size = None, 0
        for line in status.splitlines():
            if line.startswith("PPid:"):
                ppid = int(line.split()[1])
            elif line.startswith("VmRSS:"):
                size = int(line.split()[1]) * 1024
        children.setdefault(ppid, []).append(int(name))
        rss[int(name)] = size
    return children, rss


def tree_pids(table, root_pid):
    """A process and all its descendants."""
    children = table[0]
    pids, stack = [], [root_pid]
    while stack:
        pid = stack.pop()
        pids.append(pid)
        stack.extend(children.get(pid, []))
    return pids


def alive(pid):
    """Whether a process exists and is not a zombie."""
    stat = read("/proc/%d/stat" % pid)
    return bool(stat) and stat.rpartition(")")[2].split()[0] != "Z"


def list_properties():
    if not os.access(BINARY, os.X_OK):
        sys.exit("run_fuzz_all.py: build the fuzzing binary with %s/build_fuzz.sh first" % HERE)
    out = subprocess.run([BINARY, "--list_fuzz_tests"], stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, universal_newlines=True, timeout=300)
    props = re.findall(r"Fuzz test: (\S+)", out.stdout)
    if not props:
        sys.exit("run_fuzz_all.py: the fuzzing binary lists no fuzz test:\n" + out.stdout[-2000:])
    return props


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("props", nargs="*", metavar="PROP[=SECONDS]")
    parser.add_argument("--seconds", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--jobs", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--list", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.seconds is not None and args.seconds <= 0:
        parser.error("--seconds must be positive")
    if args.jobs is None:
        env = os.environ.get("FUZZ_JOBS", "")
        if env and (not env.isdigit() or int(env) == 0):
            parser.error("FUZZ_JOBS must be a positive integer")
        args.jobs = int(env) if env else 0
    elif args.jobs <= 0:
        parser.error("--jobs must be positive")
    return args


def campaign_summary(log_path):
    """Numbers from the last FuzzTest status line and the artifacts directory of a campaign log."""
    text = read(log_path) or ""
    def last(pattern):
        found = re.findall(pattern, text)
        return found[-1] if found else "-"
    return {
        "edges": last(r"Edges covered:\s*(\d+)"),
        "corpus": last(r"Corpus size:\s*(\d+)"),
        "runs_per_sec": last(r"Runs/secs:\s*(\d+)"),
        "artifacts": last(r"Artifacts: (\S+)"),
    }


def main():
    args = parse_args()
    if args.list:
        print("\n".join(list_properties()))
        return 0
    if not os.environ.get("RUST_LIB_PATH", "").startswith("/"):
        sys.exit("run_fuzz_all.py: RUST_LIB_PATH must name the Rust library by an absolute path")
    if os.environ.get("FUZZ_HARD_LIMIT_MB") == "0":
        log("WARNING: FUZZ_HARD_LIMIT_MB=0 turns off the memory limit of each campaign, and the "
            "campaigns run at the same time. Use it only when an outside container or cgroup "
            "already limits the memory (see README_fuzz.md).")

    queue = []
    for item in args.props or list_properties():
        prop, _, seconds = item.partition("=")
        if not PROP_RE.match(prop) or (seconds and not seconds.isdigit()):
            sys.exit("run_fuzz_all.py: bad property %r (use Suite.Property or Suite.Property=SECONDS)"
                     % item)
        seconds = int(seconds) if seconds else args.seconds
        queue.append((prop, seconds))

    artifacts = os.path.join(HERE, "fuzz-artifacts")
    os.makedirs(artifacts, exist_ok=True)
    batch = tempfile.mkdtemp(prefix=time.strftime("batch-%Y%m%d-%H%M%S-"), dir=artifacts)
    machine_cpus = os.cpu_count() or 1
    cpus = cpu_count()
    meter = CpuMeter()
    time.sleep(1.0)  # so that the first idle-CPU reading covers a real interval
    free, where = free_memory()
    if args.jobs:
        ceiling = args.jobs
        log("%d campaigns, at most %d at once (--jobs; no resource checks). CPUs: %d; free "
            "memory: %d MiB (%s)." % (len(queue), ceiling, cpus, free // MIB, where))
        if args.jobs > cpus:
            log("WARNING: --jobs %d is more than the %d CPUs of this command." % (args.jobs, cpus))
    else:
        ceiling = max(1, cpus - 1)
        log("%d campaigns, at most %d at once (automatic: %d CPUs; free memory %d MiB in %s). "
            "Use --jobs N to set the number yourself." % (len(queue), ceiling, cpus,
                                                          free // MIB, where))
    log("Logs and summary: %s" % batch)

    running = {}  # pid -> dict
    results = []
    peak_campaign = MIN_CAMPAIGN_BYTES

    def stop(signum, _frame):
        # Each campaign runs under `timeout`, which makes its own process group, so signal every
        # process of each campaign.
        # The processes seen at the last check count too: when the whole process group got the
        # signal, a runner can already be gone, and its campaign is no longer below it.
        table = process_table()
        pids = set(pid for job in running.values() for pid in job["pids"])
        pids.update(pid for root in running for pid in tree_pids(table, root))
        for sig in (signal.SIGTERM, signal.SIGKILL):
            for pid in pids:
                try:
                    os.kill(pid, sig)
                except OSError:
                    pass
            deadline = time.time() + 5
            while time.time() < deadline:
                for job in running.values():
                    job["proc"].poll()  # reaps the runner, so that it does not stay a zombie
                pids = [pid for pid in pids if alive(pid)]
                if not pids:
                    break
                time.sleep(0.2)
            if not pids:
                break
        log("stopped by signal %d; the running campaigns were terminated" % signum)
        sys.exit(128 + signum)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        # Keep a signal ignored, as `nohup` or a background job of a shell sets it.
        if signal.getsignal(sig) != signal.SIG_IGN:
            signal.signal(sig, stop)

    held_reason = None
    while queue or running:
        table = process_table() if running else ({}, {})
        for pid, job in list(running.items()):
            job["pids"] = tree_pids(table, pid)
            job["rss"] = sum(table[1].get(p, 0) for p in job["pids"])
            job["peak"] = max(job["peak"], job["rss"])
            peak_campaign = max(peak_campaign, job["peak"])
            code = job["proc"].poll()
            if code is None:
                continue
            del running[pid]
            job["log_file"].close()
            info = campaign_summary(job["log"])
            elapsed = int(time.time() - job["start"])
            results.append((job["prop"], code, elapsed, job["peak"], info))
            log("done  %s exit=%d %ds edges=%s corpus=%s runs/s=%s peak=%d MiB (%d running, "
                "%d waiting)" % (job["prop"], code, elapsed, info["edges"], info["corpus"],
                                 info["runs_per_sec"], job["peak"] // MIB, len(running),
                                 len(queue)))

        idle = meter.idle_cpus(machine_cpus)
        free, where = free_memory()
        # The running campaigns can still grow to the expected size of a campaign.
        growth = sum(max(0, peak_campaign - job["rss"]) for job in running.values())
        while queue and len(running) < ceiling:
            reason = None if args.jobs else wait_reason(len(running), idle, free, where,
                                                        peak_campaign, growth)
            if reason:
                # Report a wait once, not at every check.
                if reason[0] != held_reason:
                    log("waiting with %d campaigns running: %s" % (len(running), reason[1]))
                held_reason = reason[0]
                break
            held_reason = None
            prop, seconds = queue.pop(0)
            env = dict(os.environ)
            if seconds:
                env["FUZZ_DURATION_SECONDS"] = str(seconds)
            log_path = os.path.join(batch, prop + ".log")
            n = 1
            while os.path.exists(log_path):
                n += 1
                log_path = os.path.join(batch, "%s.%d.log" % (prop, n))
            log_file = open(log_path, "w")
            proc = subprocess.Popen(["bash", RUNNER, prop], stdout=log_file,
                                    stderr=subprocess.STDOUT, env=env)
            running[proc.pid] = {"prop": prop, "proc": proc, "log": log_path, "log_file": log_file,
                                 "start": time.time(), "peak": 0, "rss": 0, "pids": [proc.pid]}
            log("start %s %s(%d running, %d waiting)" % (
                prop, "%ds " % seconds if seconds else "", len(running), len(queue)))
            if not args.jobs:
                break  # one new campaign per check, so that the next check sees its load
        time.sleep(TICK_SECONDS)

    width = max([len("property")] + [len(r[0]) for r in results])
    row = "%-" + str(width) + "s %5s %6s %8s %7s %8s %9s  %s"
    lines = [row % ("property", "exit", "secs", "peak_MiB", "edges", "corpus", "runs/s",
                    "artifacts")]
    for prop, code, elapsed, peak, info in sorted(results):
        lines.append(row % (prop, code, elapsed, peak // MIB, info["edges"], info["corpus"],
                            info["runs_per_sec"], info["artifacts"]))
    failed = [r[0] for r in results if r[1] != 0]
    lines.append("ALL DONE: %d passed, %d failed%s" % (
        len(results) - len(failed), len(failed),
        " (" + " ".join(sorted(failed)) + ")" if failed else ""))
    with open(os.path.join(batch, "summary.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines), flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
