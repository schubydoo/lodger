#!/usr/bin/env python3
"""Lockfile-only fix for RustSec advisories, for ``.github/workflows/rustsec-autofix.yml``.

Usage: ``rustsec_autofix.py <out_dir>``

It runs ``cargo audit`` on ``Cargo.lock``, then ``cargo update -p <crate>@<version>`` for
every hit, then audits again. Renovate cannot do this: its OSV alerts cover direct
dependencies only, so a crate that only the lockfile pins waits for the weekly
lockFileMaintenance. ``fuzz/Cargo.lock`` is gitignored, so it is out of scope.

``cargo update -p`` stays inside the semver ranges that the manifests allow, so it never
edits a ``Cargo.toml``. An advisory whose fix needs a manifest change stays unfixed and is
reported, never forced.

Writes into ``<out_dir>``: ``body.md`` (the PR body) and ``summary.json``
(``{"fixed": [...], "remaining": [...]}``). If the lockfile changed, it also writes
``.changeset/<first advisory id>.md``. The name is per advisory, so a later run never
overwrites an unreleased fragment from an earlier run. Stdlib only. Ported from
scratchsmith's ``scripts/rustsec_autofix.py``.
"""

import json
import pathlib
import subprocess
import sys
import tomllib
from typing import NoReturn

LOCKFILE = pathlib.Path("Cargo.lock")
CHANGESETS = pathlib.Path(".changeset")


def die(msg: str) -> NoReturn:
    sys.exit(f"rustsec_autofix: {msg}")


def audit() -> list[dict]:
    """The vulnerability list of the lockfile. Warnings (unmaintained, yanked) are ignored."""
    # cargo audit exits 1 when it finds something, so the exit code is no error signal.
    # A run that produced no JSON is one.
    proc = subprocess.run(
        ["cargo", "audit", "--json", "--file", str(LOCKFILE)],
        capture_output=True,
        text=True,
        check=False,
    )
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError:
        die(f"cargo audit on {LOCKFILE} gave no JSON:\n{proc.stderr}")
    return report["vulnerabilities"]["list"]


def locked_versions(name: str) -> list[str]:
    lock = tomllib.loads(LOCKFILE.read_text())
    return sorted({p["version"] for p in lock.get("package", []) if p["name"] == name})


def main() -> None:
    if len(sys.argv) != 2:
        die("usage: rustsec_autofix.py <out_dir>")
    out = pathlib.Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)

    original = LOCKFILE.read_text()
    before = audit()
    for name, version in sorted(
        {(v["package"]["name"], v["package"]["version"]) for v in before}
    ):
        spec = f"{name}@{version}"
        upd = subprocess.run(
            ["cargo", "update", "-p", spec],
            capture_output=True,
            text=True,
            check=False,
        )
        if upd.returncode != 0:
            print(f"::warning::cargo update -p {spec} failed:\n{upd.stderr}")
    after_ids = {v["advisory"]["id"] for v in audit()}

    fixed: list[dict] = []
    remaining: list[dict] = []
    for v in before:
        row = {
            "id": v["advisory"]["id"],
            "crate": v["package"]["name"],
            "from": v["package"]["version"],
        }
        if row["id"] in after_ids:
            remaining.append(row)
        else:
            row["to"] = ", ".join(locked_versions(row["crate"]))
            fixed.append(row)

    lines = [
        (
            "Automated by `rustsec-autofix.yml`. It ran `cargo update -p <crate>@<version>` for "
            "each RustSec advisory in `Cargo.lock`. Only the lockfile changed: no `Cargo.toml` "
            "edits, and every update stays within the semver ranges that the manifests allow."
        ),
        "",
        "| Advisory | Crate | From | To |",
        "|---|---|---|---|",
    ]
    for r in sorted(fixed, key=lambda r: r["id"]):
        lines.append(
            f"| [{r['id']}](https://rustsec.org/advisories/{r['id']}) "
            f"| {r['crate']} | {r['from']} | {r['to']} |"
        )
    if remaining:
        lines += ["", "Not fixable from the lockfile (needs a manifest change):", ""]
        lines += [
            f"- {r['id']}: {r['crate']} {r['from']}"
            for r in sorted(remaining, key=lambda r: r["id"])
        ]
    (out / "body.md").write_text("\n".join(lines) + "\n")

    if not fixed:
        # An update can move a crate without a fix, for example when only a new major
        # version fixes it. Without a fix there is no fragment, so the workflow's
        # `git diff` gate must see no change.
        LOCKFILE.write_text(original)
    else:
        ids = sorted({r["id"] for r in fixed})
        crates = sorted({f"{r['crate']} {r['to']}" for r in fixed})
        # knope consumes every fragment at release, so the directory can be absent.
        CHANGESETS.mkdir(exist_ok=True)
        # scripts/lint_changesets.sh allows a one-line body only.
        (CHANGESETS / f"{ids[0].lower()}.md").write_text(
            "---\ndefault: security\n---\n\n"
            f"The lockfile moves to {', '.join(crates)}, which fixes {', '.join(ids)}.\n"
        )

    summary = {
        "fixed": sorted({r["id"] for r in fixed}),
        "remaining": sorted({r["id"] for r in remaining}),
    }
    (out / "summary.json").write_text(json.dumps(summary))
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
