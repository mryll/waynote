#!/usr/bin/env python3
"""Run isolated GTK regressions and check the input regions sent over Wayland."""

import ast
import os
from pathlib import Path
import re
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


def main():
    env = dict(os.environ, GDK_BACKEND="wayland", WAYLAND_DEBUG="client")
    result = subprocess.run(
        ["cargo", "test", "--offline", "gui_regressions", "--", "--ignored",
         "--test-threads=1", "--nocapture"],
        cwd=Path(__file__).resolve().parent.parent,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=120,
    )
    try:
        if result.returncode:
            raise AssertionError(f"GTK regression test exited with {result.returncode}")
        check_regions(result.stdout)
    except AssertionError as error:
        print(result.stdout, file=sys.stderr)
        raise SystemExit(str(error)) from error
    print("Color changes preserved edited bodies without false conflicts.")


if __name__ == "__main__":
    main()
