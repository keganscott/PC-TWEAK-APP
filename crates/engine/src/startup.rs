//! The startup apps list (CATALOGUE H12): every entry Windows starts at sign
//! in from the `Run` keys and the Startup folders, each with its switch
//! (`tweaks::startup::StartupToggle`). Reads only; turning one off or back
//! on is an ordinary apply of a toggle, through `Engine::apply`.
//!
//! Store apps' own startup tasks are listed from each installed package's
//! manifest (`AppxManifest.xml`, its `StartupTask` elements), the packages
//! from the user's `AppModel\Repository\Packages` key. VERIFY: that key and
//! its `PackageRootFolder` value (NOTES N100).
//!
//! Not listed: scheduled tasks and services, which Task Manager does not
//! list here either.
//!
//! The Startup folder paths are Windows' defaults (`FOLDERID_Startup` and
//! `FOLDERID_CommonStartup` in Microsoft's KNOWNFOLDERID reference, checked
//! 2026-10-08); a user whose Startup folder was moved elsewhere has it listed
//! from the default place (NOTES N82).

use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use crate::cleanup::Places;
use crate::context::ContextResolver;
use crate::engine::TweakView;
use crate::error::{EngineError, Result};
use crate::tweaks::startup::{StartupSource, StartupToggle, SOURCES};
use crate::types::RegRoot;

/// The signed-in user's installed packages, one subkey per package full
/// name, each with the folder it is installed in.
pub const PACKAGES: &str =
    r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

/// The two Startup folders. `None` when not known (not Windows, or the
/// profile could not be found).
#[derive(Debug, Clone, Default)]
pub struct StartupFolders {
    pub user: Option<PathBuf>,
    pub machine: Option<PathBuf>,
}

impl StartupFolders {
    /// Under the interactive user's profile, and under ProgramData.
    pub fn from_places(places: &Places) -> Self {
        Self {
            user: places
                .profile
                .as_ref()
                .map(|p| p.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup")),
            machine: places
                .program_data
                .as_ref()
                .map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs\StartUp")),
        }
    }
}

/// One startup entry as the app shows it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StartupApp {
    /// Its switch: id, state (default = starts at sign in; applied or
    /// foreign = turned off), and why it is not offered, if it is not.
    pub tweak: TweakView,
    /// The switch that turns it back on when it was turned off outside
    /// PeakTweaks: default = turned off, so offered; applied = turned on by
    /// PeakTweaks; foreign = it starts.
    pub turn_on: TweakView,
    /// The name shown: the entry's own, a shortcut without `.lnk`.
    pub name: String,
    pub source: StartupSource,
    /// What it starts: a `Run` entry's command line. `None` for a shortcut.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StartupList {
    pub apps: Vec<StartupApp>,
    /// Places that could not be read, in plain words, so a short list is not
    /// mistaken for a complete one.
    pub problems: Vec<String>,
}

/// The files in a Startup folder, `desktop.ini` aside. A folder that does not
/// exist has none.
fn folder_entries(dir: &Path) -> std::io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.eq_ignore_ascii_case("desktop.ini") {
            out.push(name);
        }
    }
    Ok(out)
}

fn source_label(source: StartupSource) -> &'static str {
    match source {
        StartupSource::UserRun => "your startup programs in the registry",
        StartupSource::MachineRun | StartupSource::MachineRun32 => "the startup programs for everyone in the registry",
        StartupSource::UserFolder => "your Startup folder",
        StartupSource::MachineFolder => "the Startup folder for everyone",
        StartupSource::StoreApp => "your Store apps' startup tasks",
    }
}

/// Every startup entry's toggle, and what could not be read.
pub fn entries(res: &ContextResolver, folders: &StartupFolders) -> (Vec<StartupToggle>, Vec<String>) {
    let mut toggles = Vec::new();
    let mut problems = Vec::new();
    for &source in SOURCES {
        if source == StartupSource::StoreApp {
            match store_tasks(res) {
                Ok((found, unread)) => {
                    toggles.extend(found);
                    if unread > 0 {
                        problems.push(format!("{unread} of {} could not be read", source_label(source)));
                    }
                }
                Err(e) => problems.push(format!("{} could not be read: {e}", source_label(source))),
            }
            continue;
        }
        let names: Result<Vec<String>> = match source.run_key() {
            Some((root, key)) => res.value_names(root, key),
            None => {
                let dir = match source {
                    StartupSource::UserFolder => &folders.user,
                    _ => &folders.machine,
                };
                match dir {
                    Some(dir) => folder_entries(dir).map_err(|e| EngineError::Internal {
                        detail: format!("{}: {e}", dir.display()),
                    }),
                    None => Err(EngineError::Internal {
                        detail: "where it is on this PC is not known".into(),
                    }),
                }
            }
        };
        match names {
            // The key's default value is not an entry.
            Ok(names) => toggles.extend(
                names
                    .into_iter()
                    .filter(|n| !n.is_empty())
                    .map(|n| StartupToggle::new(source, &n)),
            ),
            Err(e) => problems.push(format!("{} could not be read: {e}", source_label(source))),
        }
    }
    (toggles, problems)
}

/// One `StartupTask` a package manifest declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredTask {
    pub task_id: String,
    /// Starts at sign-in until the user turns it off.
    pub enabled: bool,
    /// As written: a name, or an `ms-resource:` reference to one.
    pub display_name: Option<String>,
}

/// The startup tasks in a package manifest: every element named
/// `StartupTask` in any namespace (`uap5:`, `desktop:`).
pub fn declared_tasks(manifest: &str) -> Vec<DeclaredTask> {
    let mut out = Vec::new();
    let mut rest = manifest;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        if let Some(comment) = rest.strip_prefix("!--") {
            rest = comment.find("-->").map_or("", |e| &comment[e + 3..]);
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        let tag = &rest[..end];
        if tag.rsplit(':').next() != Some("StartupTask") {
            continue;
        }
        let attrs = attributes(&rest[end..]);
        let get = |name: &str| {
            attrs
                .iter()
                .find(|(n, _)| n.rsplit(':').next() == Some(name))
                .map(|(_, v)| v.clone())
        };
        if let Some(task_id) = get("TaskId").filter(|t| !t.is_empty() && !t.contains('\\')) {
            out.push(DeclaredTask {
                task_id,
                enabled: matches!(get("Enabled").as_deref(), Some("true" | "1")),
                display_name: get("DisplayName").filter(|d| !d.is_empty()),
            });
        }
    }
    out
}

/// A start tag's attributes, from just after its name up to its `>`.
fn attributes(mut s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        let Some(eq) = s.find('=') else { break };
        if s.starts_with('>') || s.starts_with('/') || s[..eq].contains('>') {
            break;
        }
        let name = s[..eq].trim().to_owned();
        let after = s[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let Some(close) = after[1..].find(quote) else { break };
        out.push((name, unescape(&after[1..=close])));
        s = &after[close + 2..];
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// A package's family name from its full name: `Name_Version_Arch_Resource_
/// Publisher` to `Name_Publisher`. `None` for anything not shaped so.
pub fn family_of(full_name: &str) -> Option<String> {
    let parts: Vec<&str> = full_name.split('_').collect();
    (parts.len() == 5 && parts.iter().all(|p| !p.contains('\\')) && !parts[0].is_empty() && !parts[4].is_empty())
        .then(|| format!("{}_{}", parts[0], parts[4]))
}

/// The name to show for a Store app's task: the task's own name, else the
/// package's, each resolved from the package's resources when it is a
/// reference. `None` when neither can be read as words.
fn store_label(full_name: &str, task: &DeclaredTask, package_display: Option<&str>) -> Option<String> {
    let package_name = full_name.split('_').next().unwrap_or_default();
    let resolve = |text: &str| -> Option<String> {
        let resolved = if let Some(r) = text.strip_prefix("ms-resource:") {
            let uri = if r.starts_with("//") {
                format!("ms-resource:{r}")
            } else if let Some(r) = r.strip_prefix('/') {
                format!("ms-resource://{package_name}/{r}")
            } else if r.contains('/') {
                format!("ms-resource://{package_name}/{r}")
            } else {
                format!("ms-resource://{package_name}/Resources/{r}")
            };
            load_indirect(&format!("@{{{full_name}?{uri}}}"))?
        } else if text.starts_with('@') {
            load_indirect(text)?
        } else {
            text.to_owned()
        };
        let resolved = resolved.trim().to_owned();
        (!resolved.is_empty() && !resolved.starts_with("ms-resource:")).then_some(resolved)
    };
    task.display_name
        .as_deref()
        .and_then(resolve)
        .or_else(|| package_display.and_then(resolve))
}

/// A Windows indirect string (`@{package?ms-resource://...}`) read from the
/// package's resources.
#[cfg(windows)]
fn load_indirect(source: &str) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::SHLoadIndirectString;

    let mut buf = [0u16; 512];
    // SAFETY: the source is a valid wide string for the call, the buffer is
    // ours and its length is passed with it.
    unsafe { SHLoadIndirectString(&HSTRING::from(source), &mut buf, None) }.ok()?;
    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    String::from_utf16(&buf[..len]).ok()
}

#[cfg(not(windows))]
fn load_indirect(_source: &str) -> Option<String> {
    None
}

/// Every Store app startup task of the signed-in user's packages, and how
/// many packages could not be read. An error when no package is found at
/// all: every Windows user has some, so the place itself was wrong.
fn store_tasks(res: &ContextResolver) -> Result<(Vec<StartupToggle>, usize)> {
    let packages = res.subkey_names(RegRoot::InteractiveUser, PACKAGES)?;
    if packages.is_empty() {
        return Err(EngineError::Internal {
            detail: "no installed packages were found where Windows lists them".into(),
        });
    }
    let mut seen = std::collections::HashSet::new();
    let mut toggles = Vec::new();
    let mut unread = 0;
    for full_name in packages {
        let Some(family) = family_of(&full_name) else { continue };
        let key = format!(r"{PACKAGES}\{full_name}");
        let text = |value: &str| {
            res.read_raw(RegRoot::InteractiveUser, &key, value)
                .map(|v| v.and_then(|v| v.as_sz()))
        };
        let Ok(Some(root)) = text("PackageRootFolder") else {
            unread += 1;
            continue;
        };
        let manifest = match std::fs::read(Path::new(&root).join("AppxManifest.xml")) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            // A package with no manifest of its own (some framework parts).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                unread += 1;
                continue;
            }
        };
        let display = text("DisplayName").ok().flatten();
        for task in declared_tasks(&manifest) {
            let name = format!(r"{family}\{}", task.task_id);
            if !crate::tweaks::startup::is_store_task_name(&name) || !seen.insert(name.to_ascii_lowercase()) {
                continue;
            }
            let label = store_label(&full_name, &task, display.as_deref());
            toggles.push(StartupToggle::new(StartupSource::StoreApp, &name).with_store_facts(label, task.enabled));
        }
    }
    Ok((toggles, unread))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_tasks_are_read_from_a_manifest_in_any_namespace() {
        let manifest = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns:uap5="http://schemas.microsoft.com/appx/manifest/uap/windows10/5">
  <!-- <uap5:StartupTask TaskId="Commented" Enabled="true"/> -->
  <Applications><Application Id="App">
    <Extensions>
      <uap5:Extension Category="windows.startupTask" Executable="Slack.exe" EntryPoint="Windows.FullTrustApplication">
        <uap5:StartupTask TaskId="SlackStartup" Enabled="true" DisplayName="Slack &amp; Co" />
      </uap5:Extension>
      <desktop:Extension Category='windows.startupTask'>
        <desktop:StartupTask TaskId='Tray' Enabled='false'/>
      </desktop:Extension>
      <uap5:StartupTask TaskId="Bad\Id" Enabled="true"/>
      <uap5:StartupTaskOther TaskId="NotOne"/>
    </Extensions>
  </Application></Applications>
</Package>"#;
        let tasks = declared_tasks(manifest);
        let ids: Vec<&str> = tasks.iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(ids, ["SlackStartup", "Tray"]);
        assert_eq!(
            tasks[0],
            DeclaredTask {
                task_id: "SlackStartup".into(),
                enabled: true,
                display_name: Some("Slack & Co".into()),
            }
        );
        assert!(!tasks[1].enabled);
        assert_eq!(tasks[1].display_name, None);
    }

    #[test]
    fn a_package_family_comes_from_its_full_name() {
        assert_eq!(
            family_of("91750D7E.Slack_4.41.105.0_x64__8she8kybcnzg4").as_deref(),
            Some("91750D7E.Slack_8she8kybcnzg4")
        );
        assert_eq!(family_of("NotAPackage"), None);
        assert_eq!(family_of("a_b_c_d_"), None);
    }

    #[test]
    fn a_task_is_shown_by_its_own_name_then_its_package_name() {
        let task = |d: Option<&str>| DeclaredTask {
            task_id: "T".into(),
            enabled: true,
            display_name: d.map(str::to_owned),
        };
        let full = "Pkg_1.0.0.0_x64__pub";
        assert_eq!(
            store_label(full, &task(Some("Slack")), Some("Other")).as_deref(),
            Some("Slack")
        );
        assert_eq!(store_label(full, &task(None), Some("Slack")).as_deref(), Some("Slack"));
        // A reference that cannot be resolved here is never shown as is.
        assert_eq!(store_label(full, &task(Some("ms-resource:Name")), None), None);
        assert_eq!(store_label(full, &task(None), Some("  ")), None);
    }
}
