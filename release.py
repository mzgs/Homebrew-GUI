#!/usr/bin/env python3
"""Tag the next patch release for GitHub Actions to build and publish."""

import argparse
import http.client
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parent


def run(*command, capture=False):
    result = subprocess.run(command, text=True, capture_output=capture)
    if result.returncode:
        # Git errors can echo the origin URL, which contains the token.
        raise RuntimeError(f"{command[0]} {command[1]} failed (exit {result.returncode}).")
    return (result.stdout or "").strip()


def git(*args):
    return run("git", *args, capture=True)


def credentials(origin):
    url = urlsplit(origin)
    repo = url.path.removeprefix("/").removesuffix(".git")
    if url.scheme != "https" or url.hostname != "github.com" or not re.fullmatch(
        r"[\w.-]+/[\w.-]+", repo, flags=re.ASCII
    ):
        raise RuntimeError("origin must be an HTTPS github.com repository URL with a token.")
    username, password = unquote(url.username or ""), unquote(url.password or "")
    token = username if password in ("", "x-oauth-basic") else password
    if not token or token in ("x-access-token", "oauth2"):
        raise RuntimeError("No GitHub token found in origin in .git/config.")
    if not re.fullmatch(r"[!-~]+", token):
        raise RuntimeError("The GitHub token must contain printable ASCII without whitespace.")
    return repo, token


def next_version(current, tags):
    if not re.fullmatch(r"\d+\.\d+\.\d+", current):
        raise RuntimeError("Cargo.toml must have a stable major.minor.patch version.")
    versions = [tuple(map(int, current.split(".")))]
    for tag in tags.splitlines():
        if re.fullmatch(r"v?\d+\.\d+\.\d+", tag):
            versions.append(tuple(map(int, tag.removeprefix("v").split("."))))
    major, minor, patch = max(versions)
    return f"{major}.{minor}.{patch + 1}"


def github(token, path):
    headers = {
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "User-Agent": "homebrew-gui-release",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    connection = http.client.HTTPSConnection("api.github.com", timeout=120)
    try:
        connection.request("GET", path, headers=headers)
        response = connection.getresponse()
        result = response.read()
        if not 200 <= response.status < 300:
            raise RuntimeError(f"GitHub GET failed (HTTP {response.status}).")
        return json.loads(result) if result else None
    finally:
        connection.close()


def release():
    os.chdir(ROOT)
    if git("status", "--porcelain"):
        raise RuntimeError("Commit or stash all changes, including untracked files, first.")
    branch = git("symbolic-ref", "--short", "HEAD")
    git("var", "GIT_AUTHOR_IDENT")
    git("var", "GIT_COMMITTER_IDENT")
    repo, token = credentials(git("remote", "get-url", "origin"))
    github(token, f"/repos/{repo}")
    git("fetch", "--tags", "origin")

    manifest = ROOT / "Cargo.toml"
    lockfile = ROOT / "Cargo.lock"
    originals = {file: file.read_text() for file in (manifest, lockfile)}
    package = tomllib.loads(originals[manifest])["package"]
    version = next_version(package["version"], git("tag", "--list"))
    tag = f"v{version}"
    name = package["name"]
    if name != "homebrew-gui":
        raise RuntimeError("The release script expects the homebrew-gui package.")
    updated = {}
    for file, pattern in (
        (manifest, r'(?m)^(version = ")[^"]+("\s*)$'),
        (lockfile, rf'(\[\[package\]\]\nname = "{re.escape(name)}"\nversion = ")[^"]+(")'),
    ):
        updated[file], count = re.subn(pattern, lambda m: m[1] + version + m[2], originals[file])
        if count != 1:
            raise RuntimeError(f"Expected exactly one package version in {file.name}.")

    print(f"Queuing {tag} for GitHub Actions…", flush=True)
    committed = False
    try:
        for file, content in updated.items():
            file.write_text(content)
        git("add", "--", "Cargo.toml", "Cargo.lock")
        git("commit", "-m", f"Release {tag}")
        committed = True
        git("tag", "-a", tag, "-m", f"Release {tag}")
        git("push", "--atomic", "origin", f"HEAD:refs/heads/{branch}", f"refs/tags/{tag}")
        print(f"Pushed {tag}. Follow the build and publication: https://github.com/{repo}/actions")
    finally:
        if not committed:
            for file, content in originals.items():
                file.write_text(content)
            git("reset", "--quiet", "--", "Cargo.toml", "Cargo.lock")


if __name__ == "__main__":
    argparse.ArgumentParser(description=__doc__).parse_args()
    try:
        release()
    except (OSError, RuntimeError, ValueError, http.client.HTTPException) as error:
        print(f"Release failed: {error}", file=sys.stderr)
        print("Any release commit or tag already created is kept for recovery.", file=sys.stderr)
        sys.exit(1)
