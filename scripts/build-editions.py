#!/usr/bin/env python3
"""Build both Gongwen release executables for the current host."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory


ROOT = Path(__file__).resolve().parent.parent
BINARY = "gongwen-assistant"


def build(features):
    command = [
        "cargo", "build", "--locked", "--release", "--bin", BINARY,
        "--no-default-features", "--features", ",".join(features),
        "--message-format=json-render-diagnostics",
    ]
    executable = None
    with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, encoding="utf-8") as cargo:
        for line in cargo.stdout:
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                print(line, end="")
                continue
            if (
                record.get("reason") == "compiler-artifact"
                and record["target"]["name"] == BINARY
                and record.get("executable")
            ):
                executable = Path(record["executable"])
        if cargo.wait():
            raise subprocess.CalledProcessError(cargo.returncode, command)
    if executable is None:
        raise RuntimeError("Cargo did not report the release executable")
    return executable


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "dist" / "editions")
    parser.add_argument("--linux-dialogs", choices=("gtk", "portal"), default="gtk")
    args = parser.parse_args()
    output = args.output.resolve()
    dialogs = {"gtk": "linux-gtk-dialogs", "portal": "linux-portal-dialogs"}
    common_features = ["task-assistant", dialogs[args.linux_dialogs]]
    suffix = ".exe" if sys.platform == "win32" else ""
    output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix="gongwen-editions-", dir=output.parent) as temporary:
        staged = []
        for edition, name, features in (
            ("no-ai", f"{BINARY}-no-ai{suffix}", common_features),
            ("ai", f"{BINARY}{suffix}", [*common_features, "ai"]),
        ):
            print(f"Building {edition}", flush=True)
            executable = build(features)
            destination = Path(temporary) / name
            shutil.copy2(executable, destination)
            actual = subprocess.check_output([destination, "--build-edition"], encoding="utf-8").strip()
            if actual != edition:
                raise RuntimeError(f"Expected {edition}, got {actual!r} for {name}")
            staged.append(destination)
        output.mkdir(parents=True, exist_ok=True)
        for path in staged:
            print(path.replace(output / path.name))


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
