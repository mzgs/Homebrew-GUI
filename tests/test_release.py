import http.client
import os
from pathlib import Path
import subprocess
import tempfile
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

    def test_upload_streams_zip_to_github(self):
        with tempfile.TemporaryDirectory() as directory:
            asset = Path(directory) / "app.zip"
            asset.write_bytes(b"zip")
            connection = Mock()
            connection.getresponse.return_value.status = 201
            connection.getresponse.return_value.read.return_value = b'{"id": 12}'

            def check_request(method, path, body, headers):
                self.assertEqual(body.read(), b"zip")
                self.assertEqual(headers["Content-Length"], "3")
                self.assertEqual(headers["Authorization"], "Bearer secret")

            connection.request.side_effect = check_request
            with patch.object(http.client, "HTTPSConnection", return_value=connection) as connect:
                self.assertEqual(release.github("secret", "POST", "/assets", asset=asset), {"id": 12})
            connect.assert_called_once_with("uploads.github.com", timeout=120)
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
                return {("status", "--porcelain"): "dirty" if failure == "dirty" else "",
                        ("symbolic-ref", "--short", "HEAD"): "main",
                        ("remote", "get-url", "origin"): "https://user:secret@github.com/o/r.git",
                        ("tag", "--list"): "v0.1.4"}.get(args, "")

            def run(*args, **kwargs):
                events.append(("run", *args))
                if args[0] == "./build.sh":
                    self.assertIn('version = "0.1.5"', manifest.read_text())
                    if failure == "build":
                        raise RuntimeError("build failed")

            def github(token, method, path, data=None, asset=None):
                events.append(("api", method, path, data, asset))
                if asset and failure == "upload":
                    raise RuntimeError("upload failed")
                return {"id": 12, "html_url": "https://github.com/o/r/releases/tag/v0.1.5"}

            previous = Path.cwd()
            try:
                with patch.object(release, "ROOT", root), patch.object(release.sys, "platform", "darwin"), \
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
            if failure in ("build", "dirty"):
                self.assertEqual((manifest.read_text(), lock.read_text()), originals)
                self.assertFalse(any(e[:2] == ("git", "commit") for e in events))
            else:
                self.assertIn('version = "0.1.5"', manifest.read_text())
                self.assertIn('name = "homebrew-gui"\nversion = "0.1.5"', lock.read_text())
                self.assertIn('name = "other"\nversion = "2.0.0"', lock.read_text())
            return events

    def test_release_builds_both_targets_and_publishes_after_uploads(self):
        events = self.exercise_release()
        self.assertEqual([e[2] for e in events if e[:2] == ("run", "./build.sh")],
                         list(release.TARGETS.values()))
        self.assertIn(("git", "push", "--atomic", "origin", "HEAD:refs/heads/main", "refs/tags/v0.1.5"), events)
        api = [e for e in events if e[0] == "api"]
        self.assertTrue(api[1][3]["draft"])
        self.assertEqual([e[4].name for e in api[2:4]], [
            "homebrew-gui-v0.1.5-macos-arm64.zip", "homebrew-gui-v0.1.5-macos-x86_64.zip"])
        self.assertEqual(api[-1][1], "PATCH")
        self.assertEqual(api[-1][3], {"draft": False})

    def test_failures_restore_versions_or_keep_recovery_state(self):
        for failure in ("dirty", "build", "push", "upload"):
            with self.subTest(failure=failure):
                events = self.exercise_release(failure)
                self.assertFalse(any(e[:2] == ("api", "PATCH") for e in events))
                if failure != "upload":
                    self.assertFalse(any(e[:2] == ("api", "POST") for e in events))


if __name__ == "__main__":
    unittest.main()
