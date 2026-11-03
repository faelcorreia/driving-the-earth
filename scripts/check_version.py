#!/usr/bin/env python3
"""Mechanical check of the workspace version against the base branch.

Rules are described in CONTRIBUTING.md. The script reads
`[workspace.package] version` from the Cargo.toml of the checkout (the PR)
and from `<base>:Cargo.toml`, then fails if:

- the version is not MAJOR.MINOR.PATCH (no pre-release, no leading zeros);
- the major version is greater than 1;
- the version decreased;
- the PR changes code (crates/, godot/, Cargo.toml, Cargo.lock) without a bump;
- the bump is not exact (patch +1; minor +1 with patch reset to 0;
  major +1 with minor and patch reset to 0).

Bootstrap: while the base has no `v*` release tag, only the format and the major limit are
checked (the first release is v0.0.1).

Exit codes: 0 ok, 1 version rule violated, 2 could not read git data.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path

SEMVER_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")

# Highest allowed major version. Raising it is a deliberate project decision (for example,
# "this is now a different game", 1 -> 2). It is done by changing this constant in the same PR,
# so the change is visible in the diff and goes through the owner's review. There is no label
# or automatic escape. Minor and patch have no upper limit.
MAX_MAJOR = 1
CODE_PREFIXES = ("crates/", "godot/")
CODE_FILES = ("Cargo.toml", "Cargo.lock")

Version = tuple[int, int, int]


class GitDataError(RuntimeError):
    """Raised when the base version or the changed files cannot be read."""


def parse_version(text: str) -> Version:
    match = SEMVER_RE.match(text.strip())
    if match is None:
        raise ValueError(
            f"invalid version {text!r}: expected MAJOR.MINOR.PATCH "
            "(digits only, no leading zeros, no pre-release suffix)"
        )
    major, minor, patch = (int(part) for part in match.groups())
    return major, minor, patch


def format_version(version: Version) -> str:
    return ".".join(str(part) for part in version)


def workspace_version(cargo_toml_text: str) -> str:
    try:
        data = tomllib.loads(cargo_toml_text)
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"Cargo.toml is not valid TOML: {exc}") from exc
    try:
        return data["workspace"]["package"]["version"]
    except (KeyError, TypeError) as exc:
        raise ValueError("[workspace.package] version is missing in Cargo.toml") from exc


def is_code_change(path: str) -> bool:
    return path in CODE_FILES or path.startswith(CODE_PREFIXES)


def check(
    base_text: str,
    head_text: str,
    changed_files: list[str],
    bootstrap: bool = False,
) -> list[str]:
    """Return the list of rule violations; an empty list means the PR is valid.

    With bootstrap=True (no `v*` release tag exists on the base yet) only the format and
    the major limit are checked; the base version is not compared.
    """
    try:
        head_raw = workspace_version(head_text)
    except ValueError as exc:
        return [str(exc)]

    errors: list[str] = []
    try:
        head = parse_version(head_raw)
    except ValueError as exc:
        return [f"PR version: {exc}"]

    if head[0] > MAX_MAJOR:
        errors.append(
            f"PR version {head_raw}: major must not exceed {MAX_MAJOR} "
            "(1.0.0 is the last major version; later changes use 1.y.z)"
        )

    if bootstrap:
        return errors

    try:
        base_raw = workspace_version(base_text)
        base = parse_version(base_raw)
    except ValueError as exc:
        return errors + [f"base version: {exc}"]

    code_changed = any(is_code_change(path) for path in changed_files)

    if head < base:
        errors.append(
            f"version regressed: {format_version(base)} -> {head_raw}; versions never decrease"
        )
        return errors

    if head == base:
        if code_changed:
            errors.append(
                f"this PR changes code (crates/, godot/, Cargo.toml or Cargo.lock) "
                f"but the version is still {head_raw}; bump it "
                "(patch for fixes, minor for features or tile/cache format changes)"
            )
        return errors

    base_major, base_minor, base_patch = base
    head_major, head_minor, head_patch = head
    if head_major > base_major:
        if head_major != base_major + 1 or head_minor != 0 or head_patch != 0:
            errors.append(
                f"invalid major bump {format_version(base)} -> {head_raw}: "
                f"expected {base_major + 1}.0.0"
            )
    elif head_minor > base_minor:
        if head_minor != base_minor + 1 or head_patch != 0:
            errors.append(
                f"invalid minor bump {format_version(base)} -> {head_raw}: "
                f"expected {base_major}.{base_minor + 1}.0 (minor +1, patch reset to 0)"
            )
    elif head_patch != base_patch + 1:
        errors.append(
            f"invalid patch bump {format_version(base)} -> {head_raw}: "
            f"expected {base_major}.{base_minor}.{base_patch + 1}"
        )
    return errors


def git(repo: Path, *args: str) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            text=True,
        )
    except (subprocess.CalledProcessError, FileNotFoundError) as exc:
        stderr = getattr(exc, "stderr", "") or str(exc)
        raise GitDataError(f"git {' '.join(args)} failed: {stderr.strip()}") from exc
    return result.stdout


def run(repo: Path, base: str) -> int:
    try:
        base_text = git(repo, "show", f"{base}:Cargo.toml")
        changed = [
            line for line in git(repo, "diff", "--name-only", f"{base}...HEAD").splitlines() if line
        ]
        release_tags = [
            line for line in git(repo, "tag", "--list", "v*", "--merged", base).splitlines() if line
        ]
        head_text = (repo / "Cargo.toml").read_text(encoding="utf-8")
    except (GitDataError, OSError) as exc:
        print(f"version-check: cannot read data to compare with {base}: {exc}", file=sys.stderr)
        return 2

    bootstrap = not release_tags
    errors = check(base_text, head_text, changed, bootstrap=bootstrap)
    if errors:
        print(f"version-check: FAILED ({len(errors)} problem(s)) against {base}:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        print("See CONTRIBUTING.md, section Versioning.", file=sys.stderr)
        return 1

    head_version = workspace_version(head_text)
    if bootstrap:
        print(
            f"version-check: OK (bootstrap: no v* tag on {base} yet; "
            f"only format and major limit checked; PR version {head_version})"
        )
        return 0
    base_version = workspace_version(base_text)
    print(f"version-check: OK ({base_version} -> {head_version}, {len(changed)} file(s) changed)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", default="origin/main", help="base ref to compare with")
    parser.add_argument("--repo", default=".", help="repository root of the PR checkout")
    args = parser.parse_args(argv)
    return run(Path(args.repo), args.base)


if __name__ == "__main__":
    sys.exit(main())
