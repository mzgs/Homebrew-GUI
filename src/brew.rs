use serde::Deserialize;
use std::{
    io::{Read, pipe},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Formula,
    Cask,
}

impl Kind {
    pub(crate) fn flag(self) -> &'static str {
        match self {
            Self::Formula => "--formula",
            Self::Cask => "--cask",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Formula => "Formula",
            Self::Cask => "App / Cask",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Package {
    pub name: String,
    pub kind: Kind,
    pub description: String,
    pub version: String,
    pub installed: bool,
    pub next_version: Option<String>,
    pub pinned: bool,
}

impl Package {
    pub(crate) fn same_package(&self, other: &Self) -> bool {
        self.name == other.name && self.kind == other.kind
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Operation {
    Info,
    Homepage,
    Install,
    Reinstall,
    Upgrade,
    Cleanup,
    Uninstall,
}

impl Operation {
    pub(crate) fn command(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Homepage => "home",
            Self::Install => "install",
            Self::Reinstall => "reinstall",
            Self::Upgrade => "upgrade",
            Self::Cleanup => "cleanup",
            Self::Uninstall => "uninstall",
        }
    }

    pub(crate) fn args(self, package: Option<&Package>) -> Result<Vec<&str>, String> {
        match (self, package) {
            (Self::Cleanup, None) => Ok(vec![self.command(), "--prune=all"]),
            (Self::Upgrade, None) => Ok(vec![self.command()]),
            (Self::Cleanup, Some(_)) => Err("Cache cleanup applies to all packages.".into()),
            (_, Some(package)) if valid_name(&package.name) => Ok(vec![
                self.command(),
                package.kind.flag(),
                "--",
                &package.name,
            ]),
            (_, Some(_)) => Err("Invalid package name.".into()),
            (_, None) => Err("Choose a package for this command.".into()),
        }
    }
}

pub(crate) fn find_brew() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths)
        .map(|path| path.join("brew"))
        .chain(
            [
                "/opt/homebrew/bin/brew",
                "/usr/local/bin/brew",
                "/home/linuxbrew/.linuxbrew/bin/brew",
            ]
            .map(PathBuf::from),
        )
        .find(|path| path.is_file())
}

fn command(brew: &Path, args: &[&str]) -> Result<Command, String> {
    let mut paths = vec![brew.parent().unwrap_or(Path::new("/usr/bin")).to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    paths.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
    let mut command = Command::new(brew);
    command
        .args(args)
        .env(
            "PATH",
            std::env::join_paths(paths).map_err(|e| e.to_string())?,
        )
        .env("HOMEBREW_NO_COLOR", "1")
        .env("HOMEBREW_NO_EMOJI", "1")
        .env("TERM", "dumb")
        .env("HOMEBREW_NO_ASK", "1")
        .stdin(Stdio::null());
    Ok(command)
}

fn run(brew: &Path, args: &[&str]) -> Result<Output, String> {
    command(brew, args)?
        .output()
        .map_err(|e| format!("Couldn’t run {}: {e}", brew.display()))
}

#[cfg(target_os = "macos")]
fn askpass() -> Result<tempfile::TempPath, String> {
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let mut helper = tempfile::NamedTempFile::new()
        .map_err(|e| format!("Couldn’t create the administrator password helper: {e}"))?;
    helper
        .write_all(include_bytes!("askpass.applescript"))
        .map_err(|e| format!("Couldn’t write the administrator password helper: {e}"))?;
    helper
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("Couldn’t secure the administrator password helper: {e}"))?;
    Ok(helper.into_temp_path())
}

fn run_streamed(
    brew: &Path,
    args: &[&str],
    mut on_output: impl FnMut(&str),
) -> Result<Output, String> {
    let (mut reader, writer) =
        pipe().map_err(|e| format!("Couldn’t capture Homebrew output: {e}"))?;
    let stderr = writer.try_clone().map_err(|e| e.to_string())?;
    let mut command = command(brew, args)?;
    #[cfg(target_os = "macos")]
    let _askpass = if std::env::var_os("SUDO_ASKPASS").is_none() {
        let helper = askpass()?;
        command.env("SUDO_ASKPASS", &helper);
        Some(helper)
    } else {
        None
    };
    let mut child = command
        .stdout(writer)
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("Couldn’t run {}: {e}", brew.display()))?;
    drop(command);
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                output.extend_from_slice(&buffer[..count]);
                // shortcut: full snapshots preserve split UTF-8; decode incrementally for very large logs.
                on_output(&String::from_utf8_lossy(&output));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Couldn’t read Homebrew output: {error}"));
            }
        }
    }
    Ok(Output {
        status: child
            .wait()
            .map_err(|e| format!("Couldn’t wait for Homebrew: {e}"))?,
        stdout: output,
        stderr: Vec::new(),
    })
}

fn output_text(output: &Output) -> String {
    [
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ]
    .iter()
    .map(|s| s.trim())
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

fn failure(output: &Output) -> String {
    format!(
        "Homebrew exited with {}.\n{}",
        output.status,
        output_text(output)
    )
}

#[derive(Deserialize)]
struct Inventory {
    formulae: Vec<Formula>,
    casks: Vec<Cask>,
}

#[derive(Deserialize)]
struct Formula {
    full_name: String,
    desc: Option<String>,
    installed: Vec<InstalledVersion>,
    versions: Versions,
    #[serde(default)]
    revision: u32,
    outdated: bool,
    #[serde(default)]
    pinned: bool,
}

#[derive(Deserialize)]
struct Versions {
    stable: Option<String>,
    head: Option<String>,
}

#[derive(Deserialize)]
struct InstalledVersion {
    version: String,
}

#[derive(Deserialize)]
struct Cask {
    token: String,
    full_token: Option<String>,
    desc: Option<String>,
    installed: Option<String>,
    version: String,
    outdated: bool,
    #[serde(default)]
    pinned: bool,
}

fn parse_inventory(bytes: &[u8]) -> Result<Vec<Package>, String> {
    let inventory: Inventory = serde_json::from_slice(bytes)
        .map_err(|e| format!("Couldn’t read Homebrew’s package list: {e}"))?;
    let mut packages: Vec<_> = inventory
        .formulae
        .into_iter()
        .map(|p| Package {
            name: p.full_name,
            kind: Kind::Formula,
            description: p.desc.unwrap_or_default(),
            version: p
                .installed
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            installed: !p.installed.is_empty(),
            next_version: p
                .outdated
                .then(|| {
                    if p.installed.iter().any(|v| v.version.starts_with("HEAD")) {
                        return p.versions.head;
                    }
                    p.versions.stable.or(p.versions.head).map(|version| {
                        if p.revision > 0 {
                            format!("{version}_{}", p.revision)
                        } else {
                            version
                        }
                    })
                })
                .flatten(),
            pinned: p.pinned,
        })
        .chain(inventory.casks.into_iter().map(|p| Package {
            name: p.full_token.unwrap_or(p.token),
            kind: Kind::Cask,
            description: p.desc.unwrap_or_default(),
            installed: p.installed.is_some(),
            version: p.installed.unwrap_or_default(),
            next_version: p.outdated.then_some(p.version),
            pinned: p.pinned,
        }))
        .collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

pub(crate) fn installed(brew: &Path) -> Result<Vec<Package>, String> {
    let output = run(brew, &["info", "--json=v2", "--installed"])?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    parse_inventory(&output.stdout)
}

pub(crate) fn cache_size(brew: &Path) -> Result<String, String> {
    let output = run(brew, &["--cache"])?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    let cache = String::from_utf8(output.stdout)
        .map_err(|error| format!("Couldn’t read the Homebrew cache path: {error}"))?;
    let path = Path::new(cache.trim());
    if !path.is_absolute() {
        return Err("Homebrew returned an unexpected cache path.".into());
    }
    if !path.try_exists().map_err(|error| error.to_string())? {
        return Ok("0B".into());
    }
    let output = Command::new("/usr/bin/du")
        .arg("-sh")
        .arg(path)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("Couldn’t measure the Homebrew cache: {error}"))?;
    if !output.status.success() {
        return Err(output_text(&output));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| "Couldn’t read the Homebrew cache size.".into())
}

pub(crate) fn update_catalog(brew: &Path, on_output: impl FnMut(&str)) -> Result<(), String> {
    let output = run_streamed(brew, &["update"], on_output)?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/@+._-".contains(&c))
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

pub(crate) fn search(
    brew: &Path,
    query: &str,
    installed: &[Package],
) -> Result<Vec<Package>, String> {
    let query = query.trim();
    if query.len() > 128 || !valid_name(query) {
        return Err(
            "Enter a package name, such as wget, visual-studio-code, or python@3.14.".into(),
        );
    }
    let mut packages = Vec::new();
    for kind in [Kind::Formula, Kind::Cask] {
        let output = run(brew, &["search", kind.flag(), "--", query])?;
        if !output.status.success() {
            if String::from_utf8_lossy(&output.stderr).contains("No formulae or casks found for") {
                continue;
            }
            return Err(failure(&output));
        }
        for name in String::from_utf8_lossy(&output.stdout).split_whitespace() {
            if !valid_name(name) {
                return Err("Homebrew returned an unexpected search result.".into());
            }
            let package = Package {
                name: name.into(),
                kind,
                description: String::new(),
                version: String::new(),
                installed: false,
                next_version: None,
                pinned: false,
            };
            packages.push(
                installed
                    .iter()
                    .find(|p| p.same_package(&package))
                    .cloned()
                    .unwrap_or(package),
            );
        }
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

pub(crate) fn execute(
    brew: &Path,
    operation: Operation,
    package: Option<&Package>,
    on_output: impl FnMut(&str),
) -> Result<String, String> {
    let output = run_streamed(brew, &operation.args(package)?, on_output)?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    let text = output_text(&output);
    Ok(if text.is_empty() {
        "Homebrew finished successfully.".into()
    } else {
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn administrator_helper_is_private_compiles_and_is_removed() {
        use std::os::unix::fs::PermissionsExt;
        let helper = askpass().unwrap();
        let path = helper.to_path_buf();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let compiled = tempfile::NamedTempFile::new().unwrap();
        let output = Command::new("/usr/bin/osacompile")
            .arg("-o")
            .arg(compiled.path())
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        drop(helper);
        assert!(!path.exists());
    }

    #[test]
    fn reads_formulae_and_casks_without_confusing_versions_or_names() {
        let packages = parse_inventory(br#"{"formulae":[{"full_name":"custom/tap/tool","desc":null,"installed":[{"version":"1.0"},{"version":"2.0"}],"versions":{"stable":"2.0"},"revision":1,"outdated":true}],"casks":[{"token":"app","full_token":"custom/cask/app","desc":"An app","installed":"3.0","version":"4.0","outdated":true,"pinned":true}]}"#).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].name, "custom/cask/app");
        assert_eq!(packages[0].kind, Kind::Cask);
        assert_eq!(packages[0].version, "3.0");
        assert_eq!(packages[1].version, "1.0, 2.0");
        assert_eq!(packages[1].next_version.as_deref(), Some("2.0_1"));
        assert_eq!(packages[0].next_version.as_deref(), Some("4.0"));
        assert!(packages[0].pinned);
        let current = parse_inventory(br#"{"formulae":[{"full_name":"tool","desc":null,"installed":[{"version":"2.0"}],"versions":{"stable":"2.0"},"outdated":false}],"casks":[]}"#).unwrap();
        assert!(current[0].next_version.is_none());
        assert!(packages.iter().all(|p| p.installed));
        assert!(parse_inventory(b"{}").is_err());
        for name in ["--force", "a;touch x", "../app", "app/", "", "$(whoami)"] {
            assert!(!valid_name(name), "{name}");
        }
        for name in ["wget", "python@3.14", "user/tap/c++", "visual-studio-code"] {
            assert!(valid_name(name), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn streams_both_outputs_before_exit_without_waiting_for_a_newline() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("homebrew-gui-stream-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let script = directory.join("brew");
        std::fs::write(
            &script,
            r##"#!/bin/sh
printf 'stdout: café '
printf 'stderr: live' >&2
attempt=0
while [ ! -f "$0.observed" ] && [ "$attempt" -lt 200 ]; do
  sleep 0.01
  attempt=$((attempt + 1))
done
if [ ! -f "$0.observed" ]; then exit 8; fi
printf '\nfinished\n'
exit 9
"##,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = run_streamed(&script, &[], |text| {
            if text.contains("stdout: café stderr: live") {
                std::fs::write(script.with_extension("observed"), "").unwrap();
            }
        })
        .unwrap();
        assert_eq!(output.status.code(), Some(9));
        let error = failure(&output);
        assert!(error.contains("stdout: café stderr: live\nfinished"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn commands_keep_arguments_separate_and_report_failures() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("homebrew-gui-commands-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let script = directory.join("brew");
        std::fs::write(
            &script,
            r##"#!/bin/sh
case "$1" in
  --cache)
    if [ -f "$0.cache-error" ]; then echo 'Cache unavailable' >&2; exit 9; fi
    echo 'Homebrew notice' >&2
    printf '%s\n' "$0 cache";;
  info) printf '%s' '{"formulae":[],"casks":[]}';;
  search)
    if [ "$2" = --formula ]; then printf '%s\n' 'wget' 'wget2';
    else printf '%s\n' 'Error: No formulae or casks found for "wget".' >&2; exit 1; fi;;
  install|reinstall) printf '%s\n' "$@";;
  upgrade) echo 'Network unavailable' >&2; exit 9;;
esac
"##,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(cache_size(&script).unwrap(), "0B");
        let cache = directory.join("brew cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::write(cache.join("download"), vec![0; 2 * 1024 * 1024]).unwrap();
        assert!(cache_size(&script).unwrap().contains('M'));
        std::fs::write(script.with_extension("cache-error"), "").unwrap();
        assert!(
            cache_size(&script)
                .unwrap_err()
                .contains("Cache unavailable")
        );
        assert!(installed(&script).unwrap().is_empty());
        let results = search(&script, "wget", &[]).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            execute(&script, Operation::Install, Some(&results[0]), |_| {}).unwrap(),
            "install\n--formula\n--\nwget"
        );
        assert_eq!(
            execute(&script, Operation::Reinstall, Some(&results[0]), |_| {}).unwrap(),
            "reinstall\n--formula\n--\nwget"
        );
        let error = execute(&script, Operation::Upgrade, Some(&results[0]), |_| {}).unwrap_err();
        assert!(error.contains('9') && error.contains("Network unavailable"));
        assert!(search(&script, "--help", &[]).is_err());
        assert_eq!(Operation::Upgrade.args(None).unwrap(), ["upgrade"]);
        assert_eq!(
            Operation::Cleanup.args(None).unwrap(),
            ["cleanup", "--prune=all"]
        );
        assert!(execute(&script, Operation::Uninstall, None, |_| {}).is_err());
        assert!(execute(&script, Operation::Cleanup, Some(&results[0]), |_| {}).is_err());
        let mut invalid = results[0].clone();
        invalid.name = "--force".into();
        assert!(execute(&script, Operation::Upgrade, Some(&invalid), |_| {}).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
