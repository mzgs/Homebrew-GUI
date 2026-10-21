# Homebrew GUI

A small native Rust desktop app for your existing Homebrew installation.

- One search bar shows all installed formulae and macOS apps when empty.
- As you type, matching **Installed** and **Available packages** appear in separate sections without duplicates.
- Installed packages show their current version. Available updates have an **Update** button. Pinned packages stay pinned.
- Install or uninstall directly from the list. Removal requires confirmation.
- **Refresh** fetches Homebrew’s latest metadata and checks package versions again. Startup uses the available metadata for a quick initial list; a failed refresh keeps cached versions visible.
- Commands run in the background, one at a time. Results and errors appear in the activity panel.

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

Keep the app open while a command runs. Output is shown when it finishes. Installers that need interactive input or administrator authentication may need to be run in Terminal. Tap management, services, and bulk upgrades are outside this app’s scope.
