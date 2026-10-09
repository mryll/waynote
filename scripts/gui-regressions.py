#!/usr/bin/env python3
"""Run isolated GTK regressions and check the input regions sent over Wayland."""

import ast
import os
from pathlib import Path
import re
import signal
import subprocess
import sys


def check_regions(trace):
    regions = {}
    expected = None
    applied = None
    checked = 0
    for line in trace.splitlines():
        if "WAYNOTE_MAP_BEGIN " in line:
            expected = ast.literal_eval(line.split("WAYNOTE_MAP_BEGIN ", 1)[1])
            applied = None
        created = re.search(r"create_region\(new id wl_region[@#](\d+)\)", line)
        if created:
            regions[created[1]] = []
        added = re.search(r"wl_region[@#](\d+)\.add\(([-\d]+), ([-\d]+), ([-\d]+), ([-\d]+)\)", line)
        if added:
            regions[added[1]].append([int(value) for value in added.groups()[1:]])
        selected = re.search(r"wl_surface[@#]\d+\.set_input_region\(wl_region[@#](\d+)\)", line)
        if selected and expected is not None:
            applied = regions[selected[1]].copy()
        if "WAYNOTE_MAP_END" in line:
            wanted = [expected] if expected else []
            if applied != wanted:
                raise AssertionError(f"map {checked + 1}: expected {wanted}, sent {applied}")
            checked += 1
            expected = None
    if checked != 3:
        raise AssertionError(f"expected 3 map checks, observed {checked}")
    print("Wayland input regions passed for initial map, remap, and empty remap.")


def run_gui_test():
    repo = Path(__file__).resolve().parent.parent
    cargo = ["cargo", "test", "--offline", "gui_regressions"]
    # Build first, without a timeout: a cold test build alone can take longer
    # than the run budget below.
    subprocess.run(cargo + ["--no-run"], cwd=repo, check=True)
    env = dict(os.environ, GDK_BACKEND="wayland", WAYLAND_DEBUG="client")
    proc = subprocess.Popen(
        cargo + ["--", "--ignored", "--test-threads=1", "--nocapture"],
        cwd=repo,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )
    try:
        output, _ = proc.communicate(timeout=120)
    except subprocess.TimeoutExpired:
        # Kill the whole session: the test binary is cargo's child and would
        # otherwise outlive it, leaving a layer surface on screen.
        os.killpg(proc.pid, signal.SIGKILL)
        output, _ = proc.communicate()
        print(output, file=sys.stderr)
        raise SystemExit("GTK regression test timed out after 120 seconds")
    return proc.returncode, output


def main():
    returncode, output = run_gui_test()
    try:
        if returncode:
            raise AssertionError(f"GTK regression test exited with {returncode}")
        check_regions(output)
    except AssertionError as error:
        print(output, file=sys.stderr)
        raise SystemExit(str(error)) from error
    print("Color changes preserved edited bodies without false conflicts.")


if __name__ == "__main__":
    main()
