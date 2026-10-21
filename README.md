# Homebrew GUI

A small native Rust desktop app for your existing Homebrew installation.

- One search bar shows all installed formulae and macOS apps when empty.
- Packages are grouped into **Casks** and **Formulas**. As you type, installed and available matches appear together without duplicates.
- Installed packages have a checkmark and show their current version. Available updates have an **Update** button. Pinned packages stay pinned.
- Install directly from the list. Each installed package has a three-dot menu for reinstalling or uninstalling; removal requires confirmation. For formulae, **Install another version…** searches for available `name@version` packages to install.
- The package menu also offers **Package details…**, with Homebrew’s package summary in a scrollable dialog, and **Open homepage**, which opens the project’s website in your browser.
- **Refresh** fetches Homebrew’s latest metadata and checks package versions again. Startup uses the available metadata for a quick initial list; a failed refresh keeps cached versions visible.
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

Homebrew is detected in `PATH` and its standard Apple Silicon, Intel, and Linux locations. Nothing is installed or changed until you choose an action. Installs and upgrades can also change dependencies through normal Homebrew behavior.

Keep the app open while a command runs. Output appears as Homebrew writes it. On macOS, commands that need administrator access show a password dialog. The password goes directly to sudo and is not saved or logged. An existing `SUDO_ASKPASS` helper is respected. Installers that need other interactive input may need to be run in Terminal. Tap management, services, and bulk upgrades are outside this app’s scope.
