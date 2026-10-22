import http.client
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest
from unittest.mock import Mock, patch

import release


class ReleaseTests(unittest.TestCase):
    def test_credentials(self):
        for url in (
            "https://user:secret@github.com/owner/repo.git",
            "https://secret:x-oauth-basic@github.com/owner/repo.git",
            "https://secret@github.com/owner/repo.git",
            "https://x-access-token:secret@github.com/owner/repo.git",
        ):
            with self.subTest(url=url):
                self.assertEqual(release.credentials(url), ("owner/repo", "secret"))
        self.assertEqual(release.credentials("https://user:sec%21ret@github.com/o/r")[1], "sec!ret")
        for url in ("https://github.com/o/r", "git@github.com:o/r.git",
                    "https://secret@github.com.evil.test/o/r.git", "https://secret@github.com/o/r/extra",
                    "https://user:secret%0A@github.com/o/r"):
            with self.subTest(url=url), self.assertRaises(RuntimeError):
                release.credentials(url)

    def test_next_patch_uses_highest_stable_version(self):
        self.assertEqual(release.next_version("0.1.0", ""), "0.1.1")
        self.assertEqual(release.next_version("1.2.9", "v1.2.10\nv3.0.0-beta\nother\n1.3.4"), "1.3.5")
        self.assertEqual(release.next_version("2.0.0", "v1.9.9"), "2.0.1")
        with self.assertRaises(RuntimeError):
            release.next_version("1.0.0-beta", "")

    def test_git_error_does_not_expose_credentials(self):
        result = subprocess.CompletedProcess([], 1, "", "https://user:secret@github.com/o/r")
        with patch.object(release.subprocess, "run", return_value=result):
            with self.assertRaises(RuntimeError) as error:
                release.git("push", "origin")
        self.assertNotIn("secret", str(error.exception))

    def test_github_access_check(self):
        for status in (200, 401):
            connection = Mock()
            connection.getresponse.return_value.status = status
            connection.getresponse.return_value.read.return_value = b'{"id": 12}'
            with patch.object(http.client, "HTTPSConnection", return_value=connection) as connect:
                if status == 200:
                    self.assertEqual(release.github("secret", "/repos/o/r"), {"id": 12})
                else:
                    with self.assertRaises(RuntimeError):
                        release.github("secret", "/repos/o/r")
            connect.assert_called_once_with("api.github.com", timeout=120)
            self.assertEqual(connection.request.call_args.args, ("GET", "/repos/o/r"))
            self.assertEqual(connection.request.call_args.kwargs["headers"]["Authorization"],
                             "Bearer secret")
            connection.close.assert_called_once()

    def exercise_release(self, failure=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "Cargo.toml"
            lock = root / "Cargo.lock"
            manifest.write_text('[package]\nname = "homebrew-gui"\nversion = "0.1.0"\n')
            lock.write_text('version = 4\n\n[[package]]\nname = "homebrew-gui"\nversion = "0.1.0"\n'
                            '\n[[package]]\nname = "other"\nversion = "2.0.0"\n')
            originals = (manifest.read_text(), lock.read_text())
            events = []

            def git(*args):
                events.append(("git", *args))
                if args[:1] == ("push",) and failure == "push":
                    raise RuntimeError("push failed")
                if args[:1] == ("commit",) and failure == "commit":
                    raise RuntimeError("commit failed")
                return {("status", "--porcelain"): "dirty" if failure == "dirty" else "",
                        ("symbolic-ref", "--short", "HEAD"): "main",
                        ("remote", "get-url", "origin"): "https://user:secret@github.com/o/r.git",
                        ("tag", "--list"): "v0.1.4"}.get(args, "")

            def run(*args, **kwargs):
                self.fail(f"Local build command must not run: {args}")

            def github(token, path):
                events.append(("api", path))
                if failure == "auth":
                    raise RuntimeError("access denied")
                return {"id": 12}

            previous = Path.cwd()
            try:
                with patch.object(release, "ROOT", root), patch.object(release.sys, "platform", "linux"), \
                     patch.object(release, "git", side_effect=git), \
                     patch.object(release, "run", side_effect=run), \
                     patch.object(release, "github", side_effect=github):
                    if failure:
                        with self.assertRaises(RuntimeError):
                            release.release()
                    else:
                        release.release()
            finally:
                os.chdir(previous)
            if failure in ("commit", "dirty", "auth"):
                self.assertEqual((manifest.read_text(), lock.read_text()), originals)
            else:
                self.assertIn('version = "0.1.5"', manifest.read_text())
                self.assertIn('name = "homebrew-gui"\nversion = "0.1.5"', lock.read_text())
                self.assertIn('name = "other"\nversion = "2.0.0"', lock.read_text())
            self.assertFalse((root / "target").exists())
            return events

    def test_release_only_pushes_version_commit_and_tag(self):
        events = self.exercise_release()
        self.assertIn(("git", "commit", "-m", "Release v0.1.5"), events)
        self.assertIn(("git", "tag", "-a", "v0.1.5", "-m", "Release v0.1.5"), events)
        self.assertIn(("git", "push", "--atomic", "origin", "HEAD:refs/heads/main", "refs/tags/v0.1.5"), events)
        self.assertEqual([e for e in events if e[0] == "api"], [("api", "/repos/o/r")])

    def test_failures_restore_versions_or_keep_recovery_state(self):
        for failure in ("dirty", "auth", "commit", "push"):
            with self.subTest(failure=failure):
                events = self.exercise_release(failure)
                if failure in ("dirty", "auth"):
                    self.assertFalse(any(e[:2] == ("git", "commit") for e in events))
                if failure != "push":
                    self.assertFalse(any(e[:2] == ("git", "push") for e in events))
                else:
                    self.assertFalse(any(e[:2] == ("git", "reset") for e in events))

    def test_workflow_publishes_only_after_both_assets_upload(self):
        workflow = (release.ROOT / ".github/workflows/release.yml").read_text()
        publish = textwrap.dedent(workflow.split("      - name: Publish release\n", 1)[1]
                                  .split("        run: |\n", 1)[1])
        for failure in (None, "upload", "missing"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "dist").mkdir()
                for arch in ("arm64", "x86_64"):
                    if failure != "missing" or arch != "x86_64":
                        (root / f"dist/homebrew-gui-v0.1.5-macos-{arch}.zip").write_bytes(b"zip")
                gh = root / "gh"
                gh.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$RELEASE_TEST_LOG"\n'
                              'case "$1 $2" in\n'
                              '  "release view") exit 1 ;;\n'
                              '  "release upload") [ "$RELEASE_TEST_FAILURE" != upload ] ;;\n'
                              'esac\n')
                gh.chmod(0o700)
                log = root / "calls"
                result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", publish],
                                        cwd=root, capture_output=True, text=True,
                                        env={**os.environ, "PATH": f"{root}{os.pathsep}{os.environ['PATH']}",
                                             "GITHUB_REF_NAME": "v0.1.5",
                                             "RELEASE_TEST_LOG": str(log),
                                             "RELEASE_TEST_FAILURE": failure or ""})
                calls = log.read_text().splitlines() if log.exists() else []
                if failure:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(any(call.startswith("release edit") for call in calls))
                    if failure == "missing":
                        self.assertEqual(calls, [])
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn("--draft", calls[1])
                    self.assertTrue(calls[2].startswith("release upload v0.1.5 "))
                    self.assertEqual(calls[-1], "release edit v0.1.5 --draft=false")


if __name__ == "__main__":
    unittest.main()
