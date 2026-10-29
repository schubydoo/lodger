#!/usr/bin/env python3
"""Verify, or with ``--fix`` refresh, the release-asset sha256 pinned next to a version.

``.github/workflows/actionlint.yml`` pins actionlint by ``ACTIONLINT_VERSION`` and by the
sha256 of the downloaded tarball (``sha256sum -c`` fails the job on a mismatch). Renovate
bumps the version through the ``# renovate:`` annotation, but it cannot rewrite the sha256:
the ``github-releases`` datasource exposes the git-tag commit, not the asset's file hash.
GitHub publishes a per-asset SHA-256 (the ``digest`` field of a release asset), so this
script reads that digest and compares it with the pin.

``scripts/release/build.sh`` pins rustup-init the same way, by ``RUSTUP_VERSION`` and
``RUSTUP_SHA256``. rustup-init is not a GitHub release asset, so for that pin the script
reads the ``.sha256`` file that static.rust-lang.org publishes next to the binary.

* verify (default): a stale hash fails and the script prints the correct value. The
  ``binary-dep-pins`` workflow runs this mode.
* ``--fix``: a stale hash is rewritten in place. The self-hosted Renovate CE runs this as a
  ``postUpgradeTask`` (see ``schubydoo/renovate-config``), so the new hash lands in the
  same commit as the version bump.

Every pin must be verified against a published digest. A fetch error, a missing asset, or
an asset with no published digest fails in both modes, so a bump that cannot be verified
never passes. Stdlib only: the postUpgradeTask runs a bare ``python3`` with no virtualenv.
Ported from clauster's ``scripts/check_binary_dep_pins.py`` (the workflow-pin half only).

Exit codes: ``0`` every pin matches (or, with ``--fix``, now matches); ``1`` a pin is stale
(verify mode) or could not be fetched, located, parsed, or verified.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import re
import sys
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class WorkflowPin:
    """One ``*_VERSION`` + ``*_SHA256`` release-asset pin in a workflow file."""

    path: str  # workflow file, relative to the repo root
    owner: str
    repo: str
    version_re: re.Pattern[str]  # one group: the pinned version
    sha_re: re.Pattern[str]  # one group: the pinned 64-hex sha256 (the rewrite target)
    tag: Callable[[str], str]  # version -> release tag
    asset: Callable[[str], str]  # version -> downloaded asset name
    # version -> URL of a published "<sha256> <name>" file, for an asset that is not on a
    # GitHub release. When set, the script reads the hash from it instead of the release.
    sha_url: Callable[[str], str] | None = None


PINS: tuple[WorkflowPin, ...] = (
    # The version is bare ("1.7.12"), so the tag is "v" + version. The asset name matches
    # the download URL that the workflow's run-script builds.
    WorkflowPin(
        path=".github/workflows/actionlint.yml",
        owner="rhysd",
        repo="actionlint",
        version_re=re.compile(r'ACTIONLINT_VERSION:\s*"([^"]+)"'),
        sha_re=re.compile(r'ACTIONLINT_SHA256:\s*"([0-9a-f]{64})"'),
        tag=lambda v: f"v{v}",
        asset=lambda v: f"actionlint_{v}_linux_amd64.tar.gz",
    ),
    # The URL matches the download in build.sh. The rustup tags are bare ("1.29.2").
    WorkflowPin(
        path="scripts/release/build.sh",
        owner="rust-lang",
        repo="rustup",
        version_re=re.compile(r"^RUSTUP_VERSION=(\S+)$", re.MULTILINE),
        sha_re=re.compile(r"^RUSTUP_SHA256=([0-9a-f]{64})$", re.MULTILINE),
        tag=lambda v: v,
        asset=lambda _v: "rustup-init",
        sha_url=lambda v: (
            f"https://static.rust-lang.org/rustup/archive/{v}/x86_64-unknown-linux-gnu/rustup-init.sha256"
        ),
    ),
)


def fetch_text(url: str) -> str:
    """Return the body of ``url`` as text (raises on failure)."""
    with urllib.request.urlopen(url, timeout=30) as resp:  # noqa: S310 - fixed https host
        return resp.read().decode("utf-8")


def fetch_release(owner: str, repo: str, tag: str) -> dict:
    """Return the GitHub release JSON for ``owner/repo`` at ``tag`` (raises on failure)."""
    api = f"https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}"
    req = urllib.request.Request(api, headers={"Accept": "application/vnd.github+json"})  # noqa: S310
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 - fixed https api host
        return json.load(resp)


def resolve(pin: WorkflowPin, root: Path) -> tuple[str, str, str]:
    """Return ``(status, detail, current_sha)`` for one pin.

    ``status`` is ``ok`` (``detail`` is empty), ``mismatch`` (``detail`` is the published
    sha256), or ``error`` (``detail`` says why the pin could not be verified; this includes an
    asset with no published digest). ``current_sha`` is empty when the pin could not be read.
    """
    try:
        text = (root / pin.path).read_text(encoding="utf-8")
    except OSError as exc:
        return "error", f"could not read {pin.path}: {exc}", ""
    vmatch, smatch = pin.version_re.search(text), pin.sha_re.search(text)
    if vmatch is None or smatch is None:
        return "error", f"could not find the VERSION/SHA256 pin in {pin.path}", ""
    version, current = vmatch.group(1), smatch.group(1)
    tag, asset_name = pin.tag(version), pin.asset(version)
    if pin.sha_url is not None:
        url = pin.sha_url(version)
        try:
            body = fetch_text(url)
        except (OSError, http.client.HTTPException, ValueError) as exc:
            return "error", f"could not fetch {url}: {exc}", current
        published = body.split()[0] if body.split() else ""
        if re.fullmatch(r"[0-9a-f]{64}", published) is None:
            return "error", f"{url} holds no sha256: {body[:80]!r}", current
        return ("ok", "", current) if published == current else ("mismatch", published, current)
    try:
        release = fetch_release(pin.owner, pin.repo, tag)
    except (OSError, http.client.HTTPException, ValueError) as exc:
        # OSError covers urllib.error.URLError/HTTPError and timeouts, HTTPException covers a
        # truncated or dropped response, and ValueError covers malformed JSON.
        return "error", f"could not fetch {pin.owner}/{pin.repo}@{tag}: {exc}", current
    assets = [a for a in release.get("assets", []) if isinstance(a, dict)]
    asset = next((a for a in assets if a.get("name") == asset_name), None)
    if asset is None:
        return "error", f"release {tag} has no asset named {asset_name}", current
    digest = asset.get("digest")
    if not (isinstance(digest, str) and digest.startswith("sha256:")):
        return (
            "error",
            f"GitHub publishes no sha256 for {asset_name}; set the *_SHA256 by hand "
            "from the release's checksums file",
            current,
        )
    published = digest.removeprefix("sha256:")
    if re.fullmatch(r"[0-9a-f]{64}", published) is None:
        return "error", f"GitHub published a malformed sha256 for {asset_name}: {digest!r}", current
    if published == current:
        return "ok", "", current
    return "mismatch", published, current


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fix", action="store_true", help="rewrite a stale sha256 in place")
    args = parser.parse_args(argv)
    root = Path(__file__).resolve().parent.parent

    failures = 0
    fixes: list[tuple[WorkflowPin, str]] = []
    for pin in PINS:
        status, detail, current = resolve(pin, root)
        if status == "ok":
            print(f"OK   {pin.path}: sha256 matches the published digest")
        elif status == "error":
            print(f"FAIL {pin.path}: {detail}")
            failures += 1
        elif args.fix:
            fixes.append((pin, detail))
            print(f"FIX  {pin.path}: sha256\n       was:  {current}\n       now:  {detail}")
        else:
            print(
                f"FAIL {pin.path}: sha256 mismatch\n"
                f"       pinned:    {current}\n"
                f"       published: {detail}   <- set the *_SHA256 in this workflow to this"
            )
            failures += 1

    if fixes and failures:
        # All or nothing: never write a subset while another pin failed to verify.
        print(f"SKIP not rewriting {len(fixes)} pin(s): {failures} pin(s) failed")
    elif fixes:
        # Rewrite each hash through the regex that located it, so only the *_SHA256 value
        # changes. Build every new file text before writing any file.
        updated: dict[Path, str] = {}
        for pin, new in fixes:
            path = root / pin.path
            text = updated.get(path) or path.read_text(encoding="utf-8")
            match = pin.sha_re.search(text)
            if match is None:
                print(f"FAIL could not locate the SHA256 pin in {pin.path} to rewrite")
                return 1
            updated[path] = text[: match.start(1)] + new + text[match.end(1) :]
        for path, text in updated.items():
            path.write_text(text, encoding="utf-8")
        print(f"wrote {len(fixes)} refreshed sha256 pin(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
