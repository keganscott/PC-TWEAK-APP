//! The startup apps list (CATALOGUE H12): every entry Windows starts at sign
//! in from the `Run` keys and the Startup folders, each with its switch
//! (`tweaks::startup::StartupToggle`). Reads only; turning one off is an
//! ordinary apply of its toggle, through `Engine::apply`.
//!
//! Not listed: Store apps' own startup tasks, scheduled tasks and services,
//! which Task Manager does not list here either (or lists elsewhere).
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
use crate::error::Result;
use crate::tweaks::startup::{StartupSource, StartupToggle, SOURCES};

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
    }
}

/// Every startup entry's toggle, and what could not be read.
pub fn entries(res: &ContextResolver, folders: &StartupFolders) -> (Vec<StartupToggle>, Vec<String>) {
    let mut toggles = Vec::new();
    let mut problems = Vec::new();
    for &source in SOURCES {
        let names: Result<Vec<String>> = match source.run_key() {
            Some((root, key)) => res.value_names(root, key),
            None => {
                let dir = match source {
                    StartupSource::UserFolder => &folders.user,
                    _ => &folders.machine,
                };
                match dir {
                    Some(dir) => folder_entries(dir).map_err(|e| crate::error::EngineError::Internal {
                        detail: format!("{}: {e}", dir.display()),
                    }),
                    None => Err(crate::error::EngineError::Internal {
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
