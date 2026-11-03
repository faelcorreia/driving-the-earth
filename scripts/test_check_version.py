"""Tests for scripts/check_version.py. Run: python3 -m unittest discover -s scripts -p 'test_*.py'"""

from __future__ import annotations

import contextlib
import io
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import check_version as cv

CODE = ["crates/tileformat/src/lib.rs"]
DOCS = ["README.md", "docs/notes.md", ".github/workflows/version-check.yml"]


def cargo_toml(version: str) -> str:
    return (
        "[workspace]\n"
        'members = ["crates/tileformat"]\n\n'
        "[workspace.package]\n"
        f'version = "{version}"\n'
    )


class ParseVersionTest(unittest.TestCase):
    def test_accepts_plain_semver(self) -> None:
        self.assertEqual(cv.parse_version("0.0.1"), (0, 0, 1))
        self.assertEqual(cv.parse_version("1.12.0"), (1, 12, 0))

    def test_rejects_malformed_versions(self) -> None:
        for bad in ["0.1", "1.0.0.0", "01.0.0", "1.0.01", "1.0.0-rc1", "1.0.0+build", "v1.0.0", "a.b.c", ""]:
            with self.subTest(version=bad):
                with self.assertRaises(ValueError):
                    cv.parse_version(bad)


class CheckRulesTest(unittest.TestCase):
    def run_check(self, base: str, head: str, changed: list[str]) -> list[str]:
        return cv.check(cargo_toml(base), cargo_toml(head), changed)

    def assertValid(self, base: str, head: str, changed: list[str]) -> None:
        errors = self.run_check(base, head, changed)
        self.assertEqual(errors, [], f"{base} -> {head} should pass")

    def assertInvalid(self, base: str, head: str, changed: list[str], fragment: str) -> None:
        errors = self.run_check(base, head, changed)
        self.assertTrue(errors, f"{base} -> {head} should fail")
        self.assertTrue(
            any(fragment in error for error in errors),
            f"expected {fragment!r} in {errors}",
        )

    def test_valid_bumps_pass(self) -> None:
        self.assertValid("0.1.0", "0.1.1", CODE)  # patch
        self.assertValid("0.1.4", "0.2.0", CODE)  # minor resets patch
        self.assertValid("0.9.3", "1.0.0", CODE)  # first major
        self.assertValid("1.0.0", "1.0.1", CODE)
        self.assertValid("1.0.2", "1.1.0", CODE)

    def test_code_change_without_bump_fails(self) -> None:
        self.assertInvalid("0.1.0", "0.1.0", CODE, "still 0.1.0")

    def test_docs_only_without_bump_passes(self) -> None:
        self.assertValid("0.1.0", "0.1.0", DOCS)
        self.assertValid("0.1.0", "0.1.0", [])

    def test_bump_without_code_change_is_allowed(self) -> None:
        self.assertValid("0.1.0", "0.1.1", DOCS)

    def test_regression_fails(self) -> None:
        self.assertInvalid("0.2.0", "0.1.9", CODE, "regressed")
        self.assertInvalid("1.0.0", "0.9.0", CODE, "regressed")

    def test_patch_skip_fails(self) -> None:
        self.assertInvalid("0.1.0", "0.1.2", CODE, "invalid patch bump")

    def test_minor_skip_fails(self) -> None:
        self.assertInvalid("0.1.0", "0.3.0", CODE, "invalid minor bump")

    def test_minor_bump_must_reset_patch(self) -> None:
        self.assertInvalid("0.1.3", "0.2.1", CODE, "invalid minor bump")

    def test_major_must_reset_minor_and_patch(self) -> None:
        self.assertInvalid("0.1.0", "1.1.0", CODE, "invalid major bump")
        self.assertInvalid("0.1.2", "1.0.1", CODE, "invalid major bump")

    def test_major_skip_fails(self) -> None:
        self.assertInvalid("0.1.0", "2.0.0", CODE, "invalid major bump")

    def test_minor_and_patch_have_no_upper_limit(self) -> None:
        self.assertEqual(cv.parse_version("1.9999.99999999"), (1, 9999, 99999999))
        self.assertValid("1.9999.99999998", "1.9999.99999999", CODE)  # huge patch
        self.assertValid("1.9999.99999999", "1.10000.0", CODE)  # huge minor rolls over
        self.assertValid("0.9999.99999999", "1.0.0", CODE)  # major 0 -> 1 from huge numbers
        self.assertValid("0.99999.0", "0.100000.0", CODE)
        self.assertInvalid("1.9999.99999999", "1.10000.1", CODE, "invalid minor bump")
        self.assertInvalid("1.9999.99999999", "2.0.0", CODE, "major must not exceed 1")

    def test_major_above_one_fails(self) -> None:
        self.assertInvalid("1.4.2", "2.0.0", CODE, "major must not exceed 1")
        self.assertInvalid("1.4.2", "2.0.0", DOCS, "major must not exceed 1")

    def test_invalid_pr_version_fails(self) -> None:
        self.assertInvalid("0.1.0", "0.1", CODE, "PR version")

    def test_invalid_base_version_fails(self) -> None:
        errors = cv.check(cargo_toml("0.1"), cargo_toml("0.1.1"), CODE)
        self.assertTrue(any("base version" in error for error in errors))

    def test_bootstrap_check_ignores_base_version(self) -> None:
        self.assertEqual(cv.check("[workspace]\n", cargo_toml("0.0.1"), CODE, bootstrap=True), [])
        self.assertEqual(cv.check(cargo_toml("9.9.9"), cargo_toml("0.0.1"), CODE, bootstrap=True), [])
        errors = cv.check(cargo_toml("0.1.0"), cargo_toml("0.1"), CODE, bootstrap=True)
        self.assertTrue(any("PR version" in error for error in errors))

    def test_missing_workspace_version_fails(self) -> None:
        errors = cv.check("[workspace]\n", cargo_toml("0.1.1"), CODE)
        self.assertTrue(any("missing" in error for error in errors))

    def test_code_path_detection(self) -> None:
        for path in ["crates/gdext/src/lib.rs", "godot/main.tscn", "Cargo.toml", "Cargo.lock"]:
            self.assertTrue(cv.is_code_change(path), path)
        for path in ["README.md", "CONTRIBUTING.md", "docs/a.md", ".github/workflows/x.yml", "scripts/check_version.py", "LICENSE"]:
            self.assertFalse(cv.is_code_change(path), path)


class GitIntegrationTest(unittest.TestCase):
    """Builds a temporary repository with a `main` base and a feature branch."""

    def make_repo(
        self,
        base_version: str,
        head_version: str,
        head_files: dict[str, str],
        release_tag: bool = True,
    ) -> Path:
        tmp = Path(tempfile.mkdtemp(prefix="version-check-"))
        self.addCleanup(shutil.rmtree, tmp, ignore_errors=True)

        def git(*args: str) -> None:
            subprocess.run(
                ["git", "-C", str(tmp), "-c", "user.name=t", "-c", "user.email=t@example.invalid", *args],
                check=True,
                capture_output=True,
            )

        git("init", "-q")
        git("symbolic-ref", "HEAD", "refs/heads/main")
        (tmp / "Cargo.toml").write_text(cargo_toml(base_version), encoding="utf-8")
        (tmp / "README.md").write_text("base\n", encoding="utf-8")
        git("add", "-A")
        git("commit", "-q", "-m", "base")
        if release_tag:
            git("tag", f"v{base_version}")
        git("checkout", "-q", "-b", "feature")
        (tmp / "Cargo.toml").write_text(cargo_toml(head_version), encoding="utf-8")
        for rel, content in head_files.items():
            target = tmp / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
        git("add", "-A")
        git("commit", "-q", "-m", "feature")
        return tmp

    def run_script(self, repo: Path, base: str = "main") -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cv.main(["--repo", str(repo), "--base", base])
        return code, out.getvalue(), err.getvalue()

    def test_code_bump_passes(self) -> None:
        repo = self.make_repo("0.1.0", "0.1.1", {"crates/tileformat/src/lib.rs": "// fix\n"})
        code, out, err = self.run_script(repo)
        self.assertEqual(code, 0, err)
        self.assertIn("OK (0.1.0 -> 0.1.1", out)

    def test_code_without_bump_fails(self) -> None:
        repo = self.make_repo("0.1.0", "0.1.0", {"crates/tileformat/src/lib.rs": "// fix\n"})
        code, _, err = self.run_script(repo)
        self.assertEqual(code, 1)
        self.assertIn("FAILED", err)
        self.assertIn("still 0.1.0", err)

    def test_docs_without_bump_passes(self) -> None:
        repo = self.make_repo("0.1.0", "0.1.0", {"docs/notes.md": "x\n"})
        code, _, err = self.run_script(repo)
        self.assertEqual(code, 0, err)

    def test_regression_fails(self) -> None:
        repo = self.make_repo("0.2.0", "0.1.0", {"godot/main.tscn": "x\n"})
        code, _, err = self.run_script(repo)
        self.assertEqual(code, 1)
        self.assertIn("regressed", err)

    def test_minor_bump_with_format_change_passes(self) -> None:
        repo = self.make_repo("0.1.4", "0.2.0", {"crates/tileformat/src/lib.rs": "// format\n"})
        code, _, err = self.run_script(repo)
        self.assertEqual(code, 0, err)

    def test_bootstrap_without_tag_skips_regression_and_bump_rules(self) -> None:
        # No v* tag on base: a fresh 0.0.1 is accepted even though base says 0.1.0.
        repo = self.make_repo("0.1.0", "0.0.1", {"crates/tileformat/src/lib.rs": "// first\n"}, release_tag=False)
        code, out, err = self.run_script(repo)
        self.assertEqual(code, 0, err)
        self.assertIn("bootstrap", out)

    def test_bootstrap_without_tag_still_checks_format_and_major(self) -> None:
        repo = self.make_repo("0.1.0", "2.0.0", {"crates/tileformat/src/lib.rs": "// x\n"}, release_tag=False)
        code, _, err = self.run_script(repo)
        self.assertEqual(code, 1)
        self.assertIn("major must not exceed 1", err)

    def test_bootstrap_without_tag_rejects_bad_format(self) -> None:
        repo = self.make_repo("0.1.0", "0.0.1.1", {"crates/tileformat/src/lib.rs": "// x\n"}, release_tag=False)
        code, _, err = self.run_script(repo)
        self.assertEqual(code, 1)
        self.assertIn("invalid version", err)

    def test_with_tag_regression_is_checked_again(self) -> None:
        repo = self.make_repo("0.1.0", "0.0.1", {"crates/tileformat/src/lib.rs": "// x\n"}, release_tag=True)
        code, _, err = self.run_script(repo)
        self.assertEqual(code, 1)
        self.assertIn("regressed", err)

    def test_missing_base_ref_exits_2(self) -> None:
        repo = self.make_repo("0.1.0", "0.1.1", {"crates/tileformat/src/lib.rs": "// fix\n"})
        code, _, err = self.run_script(repo, base="origin/does-not-exist")
        self.assertEqual(code, 2)
        self.assertIn("cannot read data", err)


if __name__ == "__main__":
    unittest.main()
