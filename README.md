# Homebrew GUI

A small native Rust desktop app for your existing Homebrew installation.

- One search bar shows all installed formulae and macOS apps when empty.
- Packages are grouped into **Casks** and **Formulas**. As you type, installed and available matches appear together without duplicates.
- Installed packages have a checkmark and show their current version. Available updates have an **Update** button. Pinned packages stay pinned.
- Install directly from the list. Each installed package has a three-dot menu for reinstalling or uninstalling; removal requires confirmation. For formulae, **Install another version…** searches for available `name@version` packages to install.
- The package menu also offers **Package details…**, with Homebrew’s package summary in a scrollable dialog, and **Open homepage**, which opens the project’s website in your browser.
- **Refresh** fetches Homebrew’s latest metadata and checks package versions again. Startup uses the available metadata for a quick initial list; a failed refresh keeps cached versions visible.
- **Upgrade all** upgrades outdated, unpinned packages. **Clean cache** shows the cache’s total disk usage and runs `brew cleanup --prune=all` to remove cached downloads and old package versions.
- Commands run in the background, one at a time. Live output and errors appear in the activity panel, which scrolls to the latest output.

Requires [Homebrew](https://brew.sh) and stable Rust. Uses Homebrew’s [CLI and JSON output](https://docs.brew.sh/Manpage).

Run from source:

```sh
./run.sh
```

Build a macOS app:

```sh
./build.sh
open target/release/homebrew-gui.app
```

Run checks:

```sh
cargo test --locked
```

Start a GitHub release (Python 3.11+ and Git; no local Rust or macOS build tools needed):

```sh
python3 release.py
```

Commit all changes, including `.github/workflows/release.yml`, first, and enable
GitHub Actions in the repository. The script reads the GitHub token already embedded in the
HTTPS `origin` URL in `.git/config`; it does not print or copy the token. The token
needs repository contents write access and permission to push workflow files
(`workflow` scope for a classic token). It fetches tags, increments the highest
stable version from `Cargo.toml` and the tags by one patch (for example, `0.1.0` →
`0.1.1`), updates `Cargo.toml` and `Cargo.lock`, and commits the version bump.

The script pushes the current branch and the new tag atomically to `origin`,
then prints the GitHub Actions link. The **Release** workflow runs the Python
and Rust tests and builds separate Apple Silicon (`arm64`) and Intel (`x86_64`)
app ZIPs on [GitHub-hosted macOS runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Bundle versions come from `Cargo.toml`.
After both builds pass, it uploads both ZIPs and publishes **Homebrew GUI vVERSION**
with generated release notes. A release stays a draft until both ZIPs are uploaded.
Publishing uses GitHub's automatic `GITHUB_TOKEN` with contents write permission;
no extra repository secrets are needed. No builds or ZIPs are created locally,
and no local GitHub CLI or Python packages are required.

`./build.sh aarch64-apple-darwin` and `./build.sh x86_64-apple-darwin` can also build
each architecture separately under `target/<target>/release/`.

Failures before the version commit restore the original version files. After a
release commit has been created, failures keep the commit and tag for recovery;
retry the atomic push if needed. For build or publication failures, fix the cause
and rerun the workflow in GitHub Actions; publication can resume an existing draft.
Source fixes require a new commit and release tag. Builds are ad-hoc signed, without Apple
notarization, so downloaded apps may require approval in macOS Privacy & Security.

Check the release script without publishing anything:

```sh
python3 -m unittest discover -s tests
```

Homebrew is detected in `PATH` and its standard Apple Silicon, Intel, and Linux locations. Nothing is installed or changed until you choose an action. Installs and upgrades can also change dependencies through normal Homebrew behavior.

Keep the app open while a command runs. Output appears as Homebrew writes it. On macOS, commands that need administrator access show a password dialog. The password goes directly to sudo and is not saved or logged. An existing `SUDO_ASKPASS` helper is respected. Installers that need other interactive input may need to be run in Terminal. Tap management and services are outside this app’s scope.
