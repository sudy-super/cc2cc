#!/usr/bin/env python3
"""Collect the license texts shipped with locked Rust dependencies."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

root = Path(__file__).resolve().parents[1]
metadata = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
)
lines = [
    "# Third-party notices\n",
    "cc2cc is distributed under the MIT license. Dependencies retain their own licenses.\n",
]
selected = set()
for target in ("aarch64-apple-darwin", "x86_64-unknown-linux-musl"):
    tree = subprocess.run(
        [
            "cargo",
            "tree",
            "--locked",
            "--target",
            target,
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    for row in tree.splitlines():
        name, version, *_ = row.split()
        selected.add((name, version.removeprefix("v")))
for package in sorted(
    metadata["packages"], key=lambda item: (item["name"], item["version"])
):
    if (
        package["name"] == "cc2cc"
        or (package["name"], package["version"]) not in selected
    ):
        continue
    directory = Path(package["manifest_path"]).parent
    candidates = []
    for path in directory.iterdir():
        if path.is_file() and path.name.lower().startswith(
            ("license", "licence", "copyright", "notice")
        ):
            candidates.append(path)
    if package.get("license_file"):
        candidates.append(directory / package["license_file"])
    if not candidates:
        raise RuntimeError(f"License text missing: {package['name']}")
    lines.append(
        f"## {package['name']} {package['version']}\n\nLicense: {package['license']}\n"
    )
    for path in sorted(set(candidates)):
        lines.append(f"### {path.name}\n\n```text\n{path.read_text().rstrip()}\n```\n")
lines.append(
    """## ClawCodex compatibility\n
The compatibility implementation follows the published behavior of ClawCodex.
https://github.com/agentforce314/clawcodex/tree/17c950b9a59478dd867c49c5b2584ce435234b1e
The following upstream notice is retained for the compatibility reference and test oracle.\n
```text
"""
    + (root / "LICENSE")
    .read_text()
    .replace("Copyright (c) 2026 sudy-super", "Copyright (c) 2026 Clawd Codex Team")
    .rstrip()
    + "\n```\n"
)
(root / "THIRD_PARTY_NOTICES.md").write_text("\n".join(lines))
print(f"Collected notices for {len(selected) - 1} Rust dependencies")
