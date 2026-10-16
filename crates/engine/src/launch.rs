//! Starting a game from PeakTweaks (new ideas #4), for the games found in a
//! Steam library.
//!
//! PeakTweaks runs elevated, and anything it starts itself inherits its
//! administrator rights. A game must not: it gets no reason to run as an
//! administrator, and some anti-cheats object. So the game is never a child of
//! peaktweaks.exe. PeakTweaks asks the Windows desktop shell (Explorer, which
//! runs as the signed-in user without elevation) to open Steam's own link for
//! the game, `steam://rungameid/<app id>`, and Steam starts it the way its
//! library does. If the desktop shell cannot be reached the launch fails; it
//! never falls back to starting anything itself.
//!
//! The link is built here from the app id the install probe found
//! (`game_installs.rs`), never from text the UI sends.

use super::error::{EngineError, Result};
use super::game_installs::GameInstall;

/// What the errors name, so the UI words them (`src/lib/errors.ts`).
pub const WHAT: &str = "Steam";

/// Steam's link that starts an app the way its library's Play button does.
/// Valve: https://developer.valvesoftware.com/wiki/Steam_browser_protocol
pub fn steam_link(app: u32) -> String {
    format!("steam://rungameid/{app}")
}

/// The link the Play button opens for `game_id`: only for a game found in a
/// Steam library (`installs` as the last probe left them).
pub fn link_for(installs: Option<&[GameInstall]>, game_id: &str) -> Result<String> {
    let install = installs
        .unwrap_or_default()
        .iter()
        .find(|i| i.game_id == game_id)
        .ok_or_else(|| refused(format!("{game_id} was not found on this PC")))?;
    let app = install
        .steam_app
        .ok_or_else(|| refused(format!("{} was not found in a Steam library", install.name)))?;
    Ok(steam_link(app))
}

fn refused(detail: String) -> EngineError {
    EngineError::Command {
        what: WHAT.into(),
        exit_code: None,
        detail,
    }
}

/// Open `link` through the desktop shell, as the signed-in user without
/// elevation. Steam then starts the game.
#[cfg(windows)]
pub fn open_unelevated(link: &str) -> Result<()> {
    shell::open(link)
}

#[cfg(not(windows))]
pub fn open_unelevated(_link: &str) -> Result<()> {
    Err(refused("starting a game needs Windows".into()))
}

#[cfg(windows)]
mod shell {
    //! The desktop's own shell object, reached through Explorer's window
    //! list: its `ShellExecute` runs inside Explorer, so what it starts gets
    //! Explorer's token, not ours. The documented way to start a program
    //! un-elevated from an elevated one (Raymond Chen, "How can I launch an
    //! unelevated process from my elevated process and vice versa?", The Old
    //! New Thing).

    use windows::core::{Interface, BSTR, VARIANT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, IServiceProvider, CLSCTX_LOCAL_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView, IShellWindows, SID_STopLevelBrowser,
        ShellWindows, CSIDL_DESKTOP, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::{refused, Result};

    fn failed(step: &str, e: windows::core::Error) -> super::EngineError {
        refused(format!("{step}: {e}"))
    }

    /// The desktop's shell object, or why it could not be reached.
    ///
    /// # Safety
    /// COM must be started on this thread.
    unsafe fn desktop_shell() -> Result<IShellDispatch2> {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)
            .map_err(|e| failed("could not reach the Windows desktop", e))?;
        let desktop = VARIANT::from(CSIDL_DESKTOP as i32);
        let none = VARIANT::default();
        let mut hwnd = 0i32;
        let found: IDispatch = windows
            .FindWindowSW(&desktop, &none, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
            .map_err(|e| failed("could not find the Windows desktop", e))?;
        let provider: IServiceProvider = found
            .cast()
            .map_err(|e| failed("the Windows desktop did not answer (service provider)", e))?;
        let browser: IShellBrowser = provider
            .QueryService(&SID_STopLevelBrowser)
            .map_err(|e| failed("the Windows desktop did not answer (top-level browser)", e))?;
        let view: IShellView = browser
            .QueryActiveShellView()
            .map_err(|e| failed("the Windows desktop did not answer (shell view)", e))?;
        // Asked for as IDispatch first, as in Raymond Chen's sample: the view
        // hands its background object out as an automation object.
        let background: IShellFolderViewDual = view
            .GetItemObject::<IDispatch>(SVGIO_BACKGROUND)
            .and_then(|d| d.cast())
            .map_err(|e| failed("the Windows desktop did not answer (folder view)", e))?;
        let app = background
            .Application()
            .map_err(|e| failed("the Windows desktop did not answer (shell application)", e))?;
        app.cast()
            .map_err(|e| failed("the Windows desktop did not answer (shell dispatch)", e))
    }

    /// Reach the desktop's shell object without opening anything, so a test
    /// on a real desktop can show the way there works.
    #[cfg(test)]
    pub fn reach() -> Result<()> {
        on_shell_thread(|_| Ok(()))
    }

    pub fn open(link: &str) -> Result<()> {
        let link = link.to_owned();
        on_shell_thread(move |shell| {
            // SAFETY: called on the thread that started COM, with an interface
            // it handed out.
            unsafe {
                shell
                    .ShellExecute(
                        &BSTR::from(link.as_str()),
                        &VARIANT::default(),
                        &VARIANT::default(),
                        &VARIANT::from(BSTR::from("open")),
                        &VARIANT::from(SW_SHOWNORMAL.0),
                    )
                    .map_err(|e| failed("Windows did not open Steam's link", e))
            }
        })
    }

    /// Run `f` with the desktop's shell object on a short-lived thread of its
    /// own in a single-threaded apartment, as the shell expects, so the
    /// caller's COM state never matters.
    fn on_shell_thread(f: impl FnOnce(&IShellDispatch2) -> Result<()> + Send + 'static) -> Result<()> {
        std::thread::spawn(move || {
            // SAFETY: COM is started on this new thread and stopped only after
            // every interface it handed out has been dropped (they live inside
            // the inner closure).
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .map_err(|e| failed("could not start COM", e))?;
                let result = (|| f(&desktop_shell()?))();
                CoUninitialize();
                result
            }
        })
        .join()
        .unwrap_or_else(|_| Err(refused("the launch worker stopped".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Probe;

    fn install(id: &str, steam_app: Option<u32>) -> GameInstall {
        GameInstall {
            game_id: id.into(),
            name: crate::games::name(id).into(),
            path: r"D:\Games".into(),
            drive: "D:".into(),
            disk: Probe::unknown("not looked up in tests"),
            exe: None,
            steam_app,
        }
    }

    #[test]
    fn only_a_game_found_in_a_steam_library_gets_a_link() {
        let installs = [install("cs2", Some(730)), install("fortnite", None)];
        assert_eq!(link_for(Some(&installs), "cs2").unwrap(), "steam://rungameid/730");
        let not_steam = link_for(Some(&installs), "fortnite").unwrap_err().to_string();
        assert!(not_steam.contains("not found in a Steam library"), "{not_steam}");
        let missing = link_for(Some(&installs), "apex").unwrap_err().to_string();
        assert!(missing.contains("apex was not found on this PC"), "{missing}");
        assert!(link_for(None, "cs2").is_err(), "not looked for yet");
        // The UI's text never becomes part of the link.
        assert!(link_for(Some(&installs), "730\" & calc").is_err());
    }

    /// Against this PC's desktop, opening nothing. A service session (a CI
    /// runner) may have no desktop shell; the reason is printed either way.
    #[cfg(windows)]
    #[test]
    fn reaches_the_desktop_shell_or_says_why() {
        match shell::reach() {
            Ok(()) => println!("desktop shell: reached"),
            Err(e) => {
                println!("desktop shell: {e}");
                assert!(matches!(e, EngineError::Command { .. }), "{e:?}");
            }
        }
    }
}
