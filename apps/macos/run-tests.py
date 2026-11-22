#!/usr/bin/env python3
"""Run native tests with bounded, failure-preserving stall diagnostics."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def owned_rows(text, leader):
    candidates = []
    for line in text.splitlines():
        fields = line.strip().split(None, 3)
        if len(fields) != 4:
            continue
        try:
            pid, parent, pgid = map(int, fields[:3])
        except ValueError:
            continue
        candidates.append({"pid": pid, "parent": parent, "group": pgid, "executable": fields[3]})
    owned = {leader}
    for _ in range(len(candidates)):
        expanded = owned | {r["pid"] for r in candidates if r["parent"] in owned}
        if expanded == owned:
            break
        owned = expanded
    rows = [r for r in candidates if r["pid"] in owned]
    return rows


class BSDInfo(ctypes.Structure):
    # Darwin SDK sys/proc_info.h: struct proc_bsdinfo (PROC_PIDTBSDINFO=3).
    _fields_ = [("prefix", ctypes.c_uint32 * 12), ("comm", ctypes.c_char * 16),
                ("name", ctypes.c_char * 32), ("tail", ctypes.c_uint32 * 6),
                ("start_seconds", ctypes.c_uint64), ("start_microseconds", ctypes.c_uint64)]


def identity(pid):
    library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    library.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                                    ctypes.c_void_p, ctypes.c_int]
    library.proc_pidinfo.restype = ctypes.c_int
    info = BSDInfo()
    if library.proc_pidinfo(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info)) != ctypes.sizeof(info):
        return None
    if info.prefix[3] != pid or info.prefix[5] != os.getuid():
        return None
    return {"birth": [info.start_seconds, info.start_microseconds],
            "parent": info.prefix[4], "group": info.tail[1]}


def snapshot(leader):
    result = subprocess.run(
        ["/bin/ps", "-axo", "pid=,ppid=,pgid=,comm="],
        capture_output=True, text=True, timeout=5, check=True,
    )
    rows = []
    for row in owned_rows(result.stdout, leader):
        current = identity(row["pid"])
        if current and current["parent"] == row["parent"] and current["group"] == row["group"]:
            rows.append(dict(row, birth=current["birth"]))
    # A parent can exit/reparent between ps and identity checks. Do not retain
    # descendants whose verified ancestry no longer reaches our child.
    reachable = {leader}
    for _ in range(len(rows)):
        expanded = reachable | {r["pid"] for r in rows if r["parent"] in reachable}
        if expanded == reachable:
            break
        reachable = expanded
    return [r for r in rows if r["pid"] in reachable]


def signal_owned(row, action):
    current = identity(row["pid"])
    if current is None or current["birth"] != row["birth"]:
        return False
    try:
        os.kill(row["pid"], action)
        return True
    except ProcessLookupError:
        return False


def diagnose(process, destination):
    destination.mkdir(parents=True, exist_ok=True)
    try:
        rows = snapshot(process.pid)
        (destination / "owned-processes.json").write_text(json.dumps(rows, indent=2))
        # Sample only verified descendants, including Swift/Foundation children
        # that create their own process groups.
        # A bounded count keeps diagnostics inside the existing job deadline.
        for row in [r for r in rows if "xctest" in r["executable"].lower()
                    or "PackageTests" in r["executable"]][:3]:
            current = identity(row["pid"])
            if current is None or current["birth"] != row["birth"]:
                continue
            result = subprocess.run(
                ["/usr/bin/sample", str(row["pid"]), "2", "1", "-file",
                 str(destination / f"sample-{row['pid']}.txt")],
                capture_output=True, text=True, timeout=8,
            )
            (destination / f"sample-{row['pid']}-result.txt").write_text(
                f"exit={result.returncode}\n" + result.stdout[-8192:] + result.stderr[-8192:])
    except (OSError, subprocess.SubprocessError) as error:
        (destination / "diagnostic-error.txt").write_text(str(error))


def run(command, timeout, destination):
    environment = dict(os.environ, NSUnbufferedIO="YES", PYTHONUNBUFFERED="1")
    # Inherit stdout/stderr: XCTest progress is streamed, not held in a pipe.
    process = subprocess.Popen(command, env=environment, start_new_session=True)
    try:
        return process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        print(f"Native tests exceeded {timeout:g}s; collecting owned-process diagnostics", flush=True)
        try:
            diagnose(process, destination)
        finally:
            try:
                rows = snapshot(process.pid)
                (destination / "cleanup-processes.json").write_text(json.dumps(rows, indent=2))
                for row in reversed(rows):
                    signal_owned(row, signal.SIGTERM)
                time.sleep(2)
                for row in reversed(rows):
                    signal_owned(row, signal.SIGKILL)
            finally:
                # The direct child is owned even if diagnostic enumeration fails.
                # Reap only after cross-group cleanup; PID birth tokens prevent
                # signalling replacements after descendants exit/reparent.
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=2)
        return 124


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=300)
    parser.add_argument("--diagnostics-dir", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or not 0 < args.timeout <= 600:
        parser.error("provide a command and a deadline in (0, 600] seconds")
    return run(command, args.timeout, args.diagnostics_dir)


if __name__ == "__main__":
    sys.exit(main())
