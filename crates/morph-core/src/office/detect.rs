// SPDX-License-Identifier: GPL-3.0-or-later
//! Office engine detection (SPEC §3.4 "Office engine", §4.5, §7).
//!
//! - **Microsoft Office**, per app (Word, Excel, PowerPoint): the COM ProgID
//!   (`Word.Application`) → its CLSID → `LocalServer32` → the exe must exist.
//!   Click-to-Run installs are also looked up under
//!   `HKLM\SOFTWARE\Microsoft\Office\ClickToRun\REGISTRY\MACHINE\Software\Classes`.
//! - **LibreOffice**: `HKLM/HKCU\SOFTWARE\LibreOffice\LibreOffice\<version>\Path`,
//!   `…\LibreOffice\UNO\InstallPath`, then `%ProgramFiles%\LibreOffice\program`.
//!   `soffice.com` (console launcher, waits and prints) is preferred over `soffice.exe`.
//!
//! Registry reads only (a few ms); the result is cached for the process
//! ([`engines`]); [`refresh`] detects again (Settings → "detect again").
//!
//! Which engine runs a conversion: [`Engines::resolve`] with the user's
//! [`EnginePreference`] (Settings, default Auto = Microsoft Office first). A
//! preferred engine that isn't installed falls back to the other one.
//!
//! Dev/test overrides (environment): `MORPH_SOFFICE=<path to soffice.exe/.com>`,
//! `MORPH_NO_MSOFFICE=1`, `MORPH_NO_LIBREOFFICE=1` (simulate a missing engine).

use super::options::Family;
use super::win::{self, Hive, View};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Settings → Office engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnginePreference {
    /// Microsoft Office if installed, else LibreOffice.
    #[default]
    Auto,
    MicrosoftOffice,
    #[serde(rename = "libreoffice")]
    LibreOffice,
}

impl EnginePreference {
    pub const ALL: [EnginePreference; 3] = [
        EnginePreference::Auto,
        EnginePreference::MicrosoftOffice,
        EnginePreference::LibreOffice,
    ];

    /// Read `[office] engine = "auto" | "microsoft_office" | "libreoffice"` from
    /// `config.toml`. Missing file, section or bad value → `Auto`.
    pub fn from_config_file(path: &Path) -> EnginePreference {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| EnginePreference::from_config_text(&text))
            .unwrap_or_default()
    }

    pub fn from_config_text(text: &str) -> EnginePreference {
        #[derive(Deserialize, Default)]
        struct Config {
            #[serde(default)]
            office: OfficeSection,
        }
        #[derive(Deserialize, Default)]
        struct OfficeSection {
            #[serde(default)]
            engine: Option<toml::Value>,
        }
        toml::from_str::<Config>(text)
            .ok()
            .and_then(|c| c.office.engine)
            .and_then(|v| v.try_into::<EnginePreference>().ok())
            .unwrap_or_default()
    }
}

/// The two engine kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    MsOffice,
    LibreOffice,
}

/// One Microsoft Office application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OfficeApp {
    Word,
    Excel,
    PowerPoint,
}

impl OfficeApp {
    pub const ALL: [OfficeApp; 3] = [OfficeApp::Word, OfficeApp::Excel, OfficeApp::PowerPoint];

    pub fn prog_id(self) -> &'static str {
        match self {
            OfficeApp::Word => "Word.Application",
            OfficeApp::Excel => "Excel.Application",
            OfficeApp::PowerPoint => "PowerPoint.Application",
        }
    }

    /// Process image name, for finding (and killing) the instance we started.
    pub fn exe_name(self) -> &'static str {
        match self {
            OfficeApp::Word => "WINWORD.EXE",
            OfficeApp::Excel => "EXCEL.EXE",
            OfficeApp::PowerPoint => "POWERPNT.EXE",
        }
    }

    /// "Microsoft Word".
    pub fn display_name(self) -> &'static str {
        match self {
            OfficeApp::Word => "Microsoft Word",
            OfficeApp::Excel => "Microsoft Excel",
            OfficeApp::PowerPoint => "Microsoft PowerPoint",
        }
    }

    /// The app that opens a family (text-like HTML goes through Word).
    pub fn for_family(family: Family) -> OfficeApp {
        match family {
            Family::Word | Family::Text => OfficeApp::Word,
            Family::Excel => OfficeApp::Excel,
            Family::PowerPoint => OfficeApp::PowerPoint,
        }
    }
}

/// An installed Office application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub app: OfficeApp,
    pub exe: PathBuf,
    /// File version, e.g. `16.0.17928.20114`.
    pub version: Option<String>,
}

impl AppInfo {
    /// "16.0" (major.minor), for "✓ Microsoft Word 16.0 found".
    pub fn short_version(&self) -> Option<String> {
        short_version(self.version.as_deref())
    }
}

/// An installed LibreOffice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibreOfficeInfo {
    /// `soffice.com` if present (console launcher: waits and prints), else `soffice.exe`.
    pub soffice: PathBuf,
    /// File version, e.g. `26.8.1.1`.
    pub version: Option<String>,
}

impl LibreOfficeInfo {
    pub fn short_version(&self) -> Option<String> {
        short_version(self.version.as_deref())
    }
}

fn short_version(v: Option<&str>) -> Option<String> {
    let mut parts = v?.split('.');
    Some(format!("{}.{}", parts.next()?, parts.next()?))
}

/// The engine chosen for one conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineChoice {
    MsOffice(AppInfo),
    LibreOffice(LibreOfficeInfo),
}

impl EngineChoice {
    pub fn engine(&self) -> Engine {
        match self {
            EngineChoice::MsOffice(_) => Engine::MsOffice,
            EngineChoice::LibreOffice(_) => Engine::LibreOffice,
        }
    }

    /// "Microsoft Word" / "LibreOffice" (for messages like "… took too long").
    pub fn display_name(&self) -> &'static str {
        match self {
            EngineChoice::MsOffice(info) => info.app.display_name(),
            EngineChoice::LibreOffice(_) => "LibreOffice",
        }
    }
}

/// What is installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Engines {
    pub word: Option<AppInfo>,
    pub excel: Option<AppInfo>,
    pub powerpoint: Option<AppInfo>,
    pub libreoffice: Option<LibreOfficeInfo>,
}

impl Engines {
    /// Look everything up now (uncached). Prefer [`engines`].
    pub fn detect() -> Engines {
        let no_ms = env_flag("MORPH_NO_MSOFFICE");
        let ms = |app| if no_ms { None } else { detect_office_app(app) };
        Engines {
            word: ms(OfficeApp::Word),
            excel: ms(OfficeApp::Excel),
            powerpoint: ms(OfficeApp::PowerPoint),
            libreoffice: if env_flag("MORPH_NO_LIBREOFFICE") {
                None
            } else {
                detect_libreoffice()
            },
        }
    }

    pub fn office_app(&self, app: OfficeApp) -> Option<&AppInfo> {
        match app {
            OfficeApp::Word => self.word.as_ref(),
            OfficeApp::Excel => self.excel.as_ref(),
            OfficeApp::PowerPoint => self.powerpoint.as_ref(),
        }
    }

    pub fn has_ms_office(&self) -> bool {
        self.word.is_some() || self.excel.is_some() || self.powerpoint.is_some()
    }

    /// Any engine at all (else: the "missing engine" banner).
    pub fn any(&self) -> bool {
        self.has_ms_office() || self.libreoffice.is_some()
    }

    /// The engine for a family. The preferred engine first; if it isn't
    /// installed, the other one. `None`: neither is installed.
    pub fn resolve(&self, family: Family, preference: EnginePreference) -> Option<EngineChoice> {
        let ms = || {
            self.office_app(OfficeApp::for_family(family))
                .cloned()
                .map(EngineChoice::MsOffice)
        };
        let lo = || self.libreoffice.clone().map(EngineChoice::LibreOffice);
        match preference {
            EnginePreference::Auto | EnginePreference::MicrosoftOffice => ms().or_else(lo),
            EnginePreference::LibreOffice => lo().or_else(ms),
        }
    }

    /// Like [`Engines::resolve`] for PDF → Word (Word's PDF reflow, else LibreOffice).
    pub fn resolve_pdf_to_word(&self, preference: EnginePreference) -> Option<EngineChoice> {
        self.resolve(Family::Word, preference)
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0")
}

static CACHE: Mutex<Option<Arc<Engines>>> = Mutex::new(None);

/// Installed engines, detected once per process.
pub fn engines() -> Arc<Engines> {
    if let Ok(guard) = CACHE.lock()
        && let Some(cached) = guard.as_ref()
    {
        return Arc::clone(cached);
    }
    refresh()
}

/// Detect again (e.g. after the user installed LibreOffice) and update the cache.
pub fn refresh() -> Arc<Engines> {
    let fresh = Arc::new(Engines::detect());
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(Arc::clone(&fresh));
    }
    fresh
}

// ---------------------------------------------------------------- Microsoft Office

const C2R_CLASSES: &str = r"SOFTWARE\Microsoft\Office\ClickToRun\REGISTRY\MACHINE\Software\Classes";

fn detect_office_app(app: OfficeApp) -> Option<AppInfo> {
    let exe = office_server_exe(app)?;
    Some(AppInfo {
        app,
        version: win::file_version(&exe),
        exe,
    })
}

/// `LocalServer32` exe of an app's automation server, if it exists on disk.
fn office_server_exe(app: OfficeApp) -> Option<PathBuf> {
    let prog_id = app.prog_id();
    // Normal (MSI and integrated Click-to-Run) registration.
    for view in [View::Native64, View::Wow32] {
        if let Some(exe) = local_server(Hive::ClassesRoot, view, "", prog_id) {
            return Some(exe);
        }
    }
    // Click-to-Run's own copy of the classes.
    let prefix = format!(r"{C2R_CLASSES}\");
    for view in [View::Native64, View::Wow32] {
        if let Some(exe) = local_server(Hive::LocalMachine, view, &prefix, prog_id) {
            return Some(exe);
        }
    }
    None
}

fn local_server(hive: Hive, view: View, prefix: &str, prog_id: &str) -> Option<PathBuf> {
    let clsid = win::reg_string(hive, view, &format!(r"{prefix}{prog_id}\CLSID"), None)?;
    let clsid = clsid.trim();
    let command = win::reg_string(
        hive,
        view,
        &format!(r"{prefix}CLSID\{clsid}\LocalServer32"),
        None,
    )?;
    win::exe_from_command(&command).filter(|p| p.is_file())
}

// ---------------------------------------------------------------- LibreOffice

fn detect_libreoffice() -> Option<LibreOfficeInfo> {
    let soffice = std::env::var_os("MORPH_SOFFICE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .and_then(|p| program_dir_of(&p))
        .or_else(libreoffice_program_dir_from_registry)
        .or_else(libreoffice_program_dir_from_defaults)
        .and_then(|dir| soffice_in(&dir))?;
    // The version resource is on soffice.exe (soffice.com has none).
    let exe = soffice.with_extension("exe");
    Some(LibreOfficeInfo {
        version: win::file_version(&exe).or_else(|| win::file_version(&soffice)),
        soffice,
    })
}

fn program_dir_of(file: &Path) -> Option<PathBuf> {
    file.parent().map(Path::to_path_buf)
}

/// `soffice.com` if present, else `soffice.exe`.
fn soffice_in(program_dir: &Path) -> Option<PathBuf> {
    ["soffice.com", "soffice.exe"]
        .into_iter()
        .map(|name| program_dir.join(name))
        .find(|p| p.is_file())
}

fn libreoffice_program_dir_from_registry() -> Option<PathBuf> {
    for hive in [Hive::LocalMachine, Hive::CurrentUser] {
        for view in [View::Native64, View::Wow32] {
            // SOFTWARE\LibreOffice\LibreOffice\<version>\Path = …\program\soffice.exe
            let base = r"SOFTWARE\LibreOffice\LibreOffice";
            let mut versions = win::reg_subkeys(hive, view, base);
            versions.sort_by_key(|v| std::cmp::Reverse(version_key(v)));
            for v in versions {
                if let Some(path) =
                    win::reg_string(hive, view, &format!(r"{base}\{v}"), Some("Path"))
                    && let Some(dir) = program_dir_of(Path::new(&path))
                    && soffice_in(&dir).is_some()
                {
                    return Some(dir);
                }
            }
            // Older installers: UNO\InstallPath = …\program
            if let Some(dir) =
                win::reg_string(hive, view, r"SOFTWARE\LibreOffice\UNO\InstallPath", None)
                    .map(PathBuf::from)
                    .filter(|d| soffice_in(d).is_some())
            {
                return Some(dir);
            }
        }
    }
    None
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn libreoffice_program_dir_from_defaults() -> Option<PathBuf> {
    let machine = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from);
    // Per-user installs (no registry key): %LOCALAPPDATA%\Programs\LibreOffice.
    let user = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs"));
    default_program_dirs(machine.chain(user))
        .into_iter()
        .find(|dir| soffice_in(dir).is_some())
}

/// `<root>\LibreOffice\program` for each install root, in order.
fn default_program_dirs(roots: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    roots.map(|r| r.join(r"LibreOffice\program")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word() -> AppInfo {
        AppInfo {
            app: OfficeApp::Word,
            exe: PathBuf::from(r"C:\Office\WINWORD.EXE"),
            version: Some("16.0.1.2".into()),
        }
    }

    fn lo() -> LibreOfficeInfo {
        LibreOfficeInfo {
            soffice: PathBuf::from(r"C:\LO\program\soffice.com"),
            version: Some("26.8.1.1".into()),
        }
    }

    #[test]
    fn per_user_libreoffice_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let user_programs = dir.path().join("Programs");
        let program = user_programs.join(r"LibreOffice\program");
        std::fs::create_dir_all(&program).unwrap();
        std::fs::write(program.join("soffice.com"), b"").unwrap();
        let roots = [dir.path().join("Program Files"), user_programs];
        let found = default_program_dirs(roots.into_iter())
            .into_iter()
            .find(|d| soffice_in(d).is_some());
        assert_eq!(found, Some(program));
    }

    #[test]
    fn preference_order_with_fallback() {
        let both = Engines {
            word: Some(word()),
            libreoffice: Some(lo()),
            ..Engines::default()
        };
        let auto = both.resolve(Family::Word, EnginePreference::Auto).unwrap();
        assert_eq!(auto.engine(), Engine::MsOffice);
        assert_eq!(auto.display_name(), "Microsoft Word");
        assert_eq!(
            both.resolve(Family::Text, EnginePreference::LibreOffice)
                .map(|c| c.engine()),
            Some(Engine::LibreOffice)
        );
        // No Excel installed: Excel files go to LibreOffice even with "Microsoft Office".
        assert_eq!(
            both.resolve(Family::Excel, EnginePreference::MicrosoftOffice)
                .map(|c| c.engine()),
            Some(Engine::LibreOffice)
        );
        let only_word = Engines {
            word: Some(word()),
            ..Engines::default()
        };
        assert_eq!(
            only_word
                .resolve(Family::Word, EnginePreference::LibreOffice)
                .map(|c| c.engine()),
            Some(Engine::MsOffice)
        );
        assert!(
            only_word
                .resolve(Family::PowerPoint, EnginePreference::Auto)
                .is_none()
        );
        assert!(!Engines::default().any());
        assert_eq!(word().short_version().as_deref(), Some("16.0"));
        assert_eq!(lo().short_version().as_deref(), Some("26.8"));
    }

    #[test]
    fn preference_from_config() {
        assert_eq!(
            EnginePreference::from_config_text("[office]\nengine = \"libreoffice\"\n"),
            EnginePreference::LibreOffice
        );
        assert_eq!(
            EnginePreference::from_config_text("[office]\nengine = \"microsoft_office\""),
            EnginePreference::MicrosoftOffice
        );
        assert_eq!(
            EnginePreference::from_config_text("[office]\nengine = \"word97\""),
            EnginePreference::Auto
        );
        assert_eq!(
            EnginePreference::from_config_text("[menu]\nimages = true"),
            EnginePreference::Auto
        );
        assert_eq!(
            EnginePreference::from_config_file(Path::new(r"C:\no\such\config.toml")),
            EnginePreference::Auto
        );
    }

    #[test]
    fn detection_runs_and_reports() {
        let start = std::time::Instant::now();
        let found = Engines::detect();
        let elapsed = start.elapsed();
        println!("detected in {elapsed:?}: {found:#?}");
        if let Some(lo) = &found.libreoffice {
            assert!(lo.soffice.is_file());
        }
        for app in OfficeApp::ALL {
            if let Some(info) = found.office_app(app) {
                assert!(info.exe.is_file());
            }
        }
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "detection must be cheap"
        );
        assert_eq!(*engines(), *refresh());
    }
}
