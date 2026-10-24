// SPDX-License-Identifier: GPL-3.0-or-later
//! LibreOffice headless runner (SPEC §7, §10, §13).
//!
//! ```text
//! soffice.com -env:UserInstallation=file:///<private profile> --headless --invisible
//!             --norestore --nolockcheck --nodefault --nologo --nofirststartwizard
//!             [--infilter=<filter>] --convert-to <spec> --outdir <temp>\out <temp>\in\input.<ext>
//! ```
//!
//! - **Private profile**: one of a few profile folders under `<LocalData>\libreoffice\`,
//!   leased with an exclusive lock file. So we never talk to the user's running
//!   LibreOffice, and two conversions (also from two `Morph.exe` processes) never
//!   share a profile, which LibreOffice can't handle (SPEC §13).
//! - **Input is staged**: copied into the task's temp folder as `input.<ext>`, so
//!   LibreOffice never writes lock files next to the user's file and odd names /
//!   long paths can't trip it. Pass `stage_input: false` only when the file must
//!   stay next to its relative resources (and know that LibreOffice may then
//!   create a `.~lock.*#` file there while it runs).
//! - **Kill**: the process runs in a kill-on-close job object (started suspended,
//!   then resumed), so `soffice.bin` dies with it on timeout, cancel or drop.
//! - **Output**: whatever LibreOffice wrote into `out\`; publish it with
//!   [`crate::naming::publish_file`] (temp file + rename in the target folder).

use super::detect::LibreOfficeInfo;
use super::win::{self, KillJob};
use crate::converter::TaskCtx;
use crate::error::{ConvertError, ErrorKind};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

/// How many private profiles may run at once (across all Morph processes).
const PROFILE_SLOTS: u32 = 3;
/// Lines of LibreOffice output kept for "Details" (SPEC §10: last 30 lines).
const LOG_LINES: usize = 30;
/// How often we look at the process, the clock and the cancel button.
const POLL: Duration = Duration::from_millis(100);
/// After a kill, how long we wait for the whole tree to be gone before the
/// profile is handed to the next run (a still-dying `soffice.bin` holds the
/// profile and its pipe, so a new run would talk to it and fail).
const KILL_WAIT: Duration = Duration::from_secs(15);

/// One `--convert-to` run.
#[derive(Debug, Clone)]
pub struct SofficeJob<'a> {
    pub input: &'a Path,
    /// `--convert-to` value: an extension, optionally with a filter:
    /// `"pdf"`, `"docx:MS Word 2007 XML"`, `"csv:Text - txt - csv (StarCalc):44,34,76"`.
    pub convert_to: &'a str,
    /// `--infilter=` (e.g. `"HTML (StarWriter)"` to open HTML in Writer, or
    /// `"writer_pdf_import"`), `None` = let LibreOffice detect.
    pub infilter: Option<&'a str>,
    /// Copy the input into the temp folder first (recommended; see module docs).
    pub stage_input: bool,
    /// Hard limit; the process tree is killed after it.
    pub timeout: Duration,
}

impl<'a> SofficeJob<'a> {
    /// Staged input, no input filter, the default engine timeout.
    pub fn new(input: &'a Path, convert_to: &'a str) -> SofficeJob<'a> {
        SofficeJob {
            input,
            convert_to,
            infilter: None,
            stage_input: true,
            timeout: super::ENGINE_TIMEOUT,
        }
    }
}

/// What a run produced.
#[derive(Debug, Clone)]
pub struct SofficeOutput {
    /// Files LibreOffice wrote, all in one temp folder (sorted by name).
    pub files: Vec<PathBuf>,
    /// The last lines LibreOffice printed (for details).
    pub log: String,
}

impl SofficeOutput {
    /// The first output file (most conversions write exactly one).
    pub fn file(&self) -> Result<&Path, ConvertError> {
        self.files
            .first()
            .map(PathBuf::as_path)
            .ok_or_else(|| convert_failed(&self.log))
    }
}

/// Run one LibreOffice conversion. Outputs land in a fresh folder inside the
/// task's temp dir; nothing is written next to the input.
pub fn convert(
    lo: &LibreOfficeInfo,
    job: &SofficeJob<'_>,
    ctx: &TaskCtx<'_>,
) -> Result<SofficeOutput, ConvertError> {
    ctx.check_cancel()?;
    let work = ctx
        .temp_dir()?
        .join(format!("lo-{}", uuid::Uuid::new_v4().simple()));
    let in_dir = work.join("in");
    let out_dir = work.join("out");
    std::fs::create_dir_all(&in_dir)
        .and_then(|()| std::fs::create_dir_all(&out_dir))
        .map_err(|e| ConvertError::other("Couldn't create a temporary folder.", e))?;

    let input = if job.stage_input {
        let ext = job
            .input
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let staged = in_dir.join(format!("input.{ext}"));
        std::fs::copy(job.input, &staged).map_err(ConvertError::from)?;
        staged
    } else {
        job.input.to_path_buf()
    };

    // Waiting for a free profile has its own budget; the timeout counts from
    // the start of LibreOffice, so a busy machine can't make a short job time out.
    let slot_deadline = Instant::now() + job.timeout.max(super::ENGINE_TIMEOUT);
    let lease = ProfileLease::acquire(ctx, slot_deadline)?;
    seed_profile(&lease.profile_dir).map_err(|e| {
        ConvertError::other(
            "LibreOffice couldn't be prepared. Check that the disk isn't full and try again.",
            format!("profile settings: {e}"),
        )
    })?;

    let mut cmd = Command::new(&lo.soffice);
    cmd.arg(format!(
        "-env:UserInstallation={}",
        file_url(&lease.profile_dir)
    ))
    .args([
        "--headless",
        "--invisible",
        "--norestore",
        "--nolockcheck",
        "--nodefault",
        "--nologo",
        "--nofirststartwizard",
    ]);
    if let Some(filter) = job.infilter {
        cmd.arg(format!("--infilter={filter}"));
    }
    cmd.arg("--convert-to")
        .arg(job.convert_to)
        .arg("--outdir")
        .arg(&out_dir)
        .arg(&input)
        .current_dir(&work)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);

    // The lease is released only after the process tree is gone (also on kill).
    let run = run_to_end(cmd, ctx, job.timeout);
    drop(lease);
    let log = run?;

    let mut files: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    if files.is_empty() {
        // Headless LibreOffice can't ask for a password: it fails with "could
        // not be loaded". For an encrypted old file, say so plainly.
        if super::ole::legacy_encryption(job.input).is_some() {
            let mut e = super::password_protected();
            e.details = format!("{}: encrypted document\n{log}", job.input.display());
            return Err(e);
        }
        return Err(convert_failed(&log));
    }
    Ok(SofficeOutput { files, log })
}

/// Spawn suspended, put in a kill job, resume, wait (cancel / timeout kill the tree).
/// Returns the output tail.
fn run_to_end(
    mut cmd: Command,
    ctx: &TaskCtx<'_>,
    timeout: Duration,
) -> Result<String, ConvertError> {
    let started = Instant::now();
    let deadline = started + timeout;
    let mut child = cmd.spawn().map_err(|e| {
        ConvertError::new(
            ErrorKind::Other,
            "LibreOffice couldn't be started.",
            format!("{}: {e}", "soffice"),
        )
    })?;
    let job = KillJob::new();
    let in_job = job
        .as_ref()
        .is_some_and(|j| j.assign(child.as_raw_handle()));
    win::resume_process(child.id());

    let log = Arc::new(Mutex::new(VecDeque::new()));
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|stream| {
        let log = Arc::clone(&log);
        std::thread::spawn(move || collect_lines(stream, &log))
    })
    .collect();

    // TerminateJobObject is asynchronous: wait until every process is really gone.
    let stop_tree = |j: &KillJob| {
        j.terminate();
        let until = Instant::now() + KILL_WAIT;
        while in_job && j.active_processes() > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        if let Some(j) = &job {
            stop_tree(j);
        }
        let _ = child.wait();
    };

    // 1. The launcher process.
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => {
                kill(&mut child);
                return Err(ConvertError::other("LibreOffice stopped unexpectedly.", e));
            }
        }
        if ctx.cancel.is_cancelled() {
            kill(&mut child);
            return Err(ConvertError::cancelled());
        }
        if Instant::now() >= deadline {
            kill(&mut child);
            return Err(timeout_error(&tail(&log), timeout, started.elapsed()));
        }
        std::thread::sleep(POLL);
    }
    // 2. Anything it left running (soffice.bin) must finish too.
    if in_job && let Some(j) = &job {
        while j.active_processes() > 0 {
            if ctx.cancel.is_cancelled() {
                stop_tree(j);
                return Err(ConvertError::cancelled());
            }
            if Instant::now() >= deadline {
                stop_tree(j);
                return Err(timeout_error(&tail(&log), timeout, started.elapsed()));
            }
            std::thread::sleep(POLL);
        }
    }
    for r in readers {
        let _ = r.join();
    }
    Ok(tail(&log))
}

fn collect_lines(stream: Box<dyn Read + Send>, log: &Mutex<VecDeque<String>>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
                if line.is_empty() {
                    continue;
                }
                if let Ok(mut l) = log.lock() {
                    if l.len() == LOG_LINES {
                        l.pop_front();
                    }
                    l.push_back(line);
                }
            }
        }
    }
}

fn tail(log: &Mutex<VecDeque<String>>) -> String {
    log.lock()
        .map(|l| l.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// `ran` = from start until the whole tree was gone (kill included), without
/// the wait for a free profile.
fn timeout_error(log: &str, timeout: Duration, ran: Duration) -> ConvertError {
    ConvertError::other(
        "LibreOffice took too long and was stopped.",
        format!(
            "timeout after {:.1}s (stopped after {:.1}s)\n{log}",
            timeout.as_secs_f64(),
            ran.as_secs_f64()
        ),
    )
}

/// LibreOffice exited without writing anything.
pub fn convert_failed(log: &str) -> ConvertError {
    let lower = log.to_ascii_lowercase();
    if lower.contains("could not be loaded") {
        ConvertError::new(
            ErrorKind::BadInput,
            "LibreOffice couldn't open this file. It may be damaged or password-protected.",
            log,
        )
    } else {
        ConvertError::other("LibreOffice couldn't convert this file.", log)
    }
}

// ---------------------------------------------------------------- profiles

/// Root of the private profiles: `<LocalData>\libreoffice`.
pub fn profiles_root() -> PathBuf {
    morph_formats::paths::app_data_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("morph"))
        .join("libreoffice")
}

/// Exclusive use of one private profile (`profile-N`), held by an open lock
/// file nobody else can open. Released on drop (also if the process dies).
struct ProfileLease {
    profile_dir: PathBuf,
    _lock: File,
}

impl ProfileLease {
    fn acquire(ctx: &TaskCtx<'_>, deadline: Instant) -> Result<ProfileLease, ConvertError> {
        let root = profiles_root();
        std::fs::create_dir_all(&root)
            .map_err(|e| ConvertError::other("Couldn't create a temporary folder.", e))?;
        loop {
            for slot in 0..PROFILE_SLOTS {
                let lock = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .share_mode(0)
                    .open(root.join(format!("profile-{slot}.lock")));
                if let Ok(file) = lock {
                    return Ok(ProfileLease {
                        profile_dir: root.join(format!("profile-{slot}")),
                        _lock: file,
                    });
                }
            }
            ctx.check_cancel()?;
            if Instant::now() >= deadline {
                return Err(ConvertError::other(
                    "LibreOffice is busy with other files. Try again in a moment.",
                    "no free LibreOffice profile slot",
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// Settings every private profile must have, as LibreOffice itself writes them
/// in `user\registrymodifications.xcu`:
/// - macros: security level "very high" (3) and macro execution disabled,
/// - links to other files are never updated. Per the official schemas, Writer
///   `Link` is 0 = always, 1 = on request, 2 = never (so we use 2); Calc `Link`
///   is 0 = always, 1 = never, 2 = on request (so we use 1),
/// - documents may not load images and other content from untrusted links.
pub const PROFILE_SETTINGS: &[(&str, &str, &str)] = &[
    (
        "/org.openoffice.Office.Common/Security/Scripting",
        "MacroSecurityLevel",
        "3",
    ),
    (
        "/org.openoffice.Office.Common/Security/Scripting",
        "DisableMacrosExecution",
        "true",
    ),
    (
        "/org.openoffice.Office.Common/Security/Scripting",
        "BlockUntrustedRefererLinks",
        "true",
    ),
    ("/org.openoffice.Office.Writer/Content/Update", "Link", "2"),
    ("/org.openoffice.Office.Calc/Content/Update", "Link", "1"),
];

/// One `<item>` exactly as LibreOffice writes it (so it survives its rewrite
/// on exit and the "already there" check keeps working).
pub fn profile_item(path: &str, name: &str, value: &str) -> String {
    format!(
        "<item oor:path=\"{path}\"><prop oor:name=\"{name}\" oor:op=\"fuse\"><value>{value}</value></prop></item>"
    )
}

/// Make sure `profile\user\registrymodifications.xcu` has [`PROFILE_SETTINGS`]
/// (creating the file for a new profile, adding missing items otherwise).
fn seed_profile(profile_dir: &Path) -> std::io::Result<()> {
    const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<oor:items \
xmlns:oor=\"http://openoffice.org/2001/registry\" xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" \
xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n";
    const TAIL: &str = "</oor:items>";
    let user = profile_dir.join("user");
    std::fs::create_dir_all(&user)?;
    let file = user.join("registrymodifications.xcu");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let missing: String = PROFILE_SETTINGS
        .iter()
        .map(|(p, n, v)| profile_item(p, n, v))
        .filter(|item| !existing.contains(item.as_str()))
        .map(|item| item + "\n")
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    // Later items win, so appending overrides any older value of the same key.
    let text = match existing.rfind(TAIL) {
        Some(at) => format!("{}{missing}{}", &existing[..at], &existing[at..]),
        None => format!("{HEAD}{missing}{TAIL}\n"),
    };
    let temp = user.join("registrymodifications.xcu.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, &file)
}

/// `file:///C:/Users/A%20B/…` for `-env:UserInstallation` (UTF-8, percent-encoded).
pub fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let mut url = String::from("file:///");
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                url.push(b as char)
            }
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_are_encoded() {
        assert_eq!(
            file_url(Path::new(r"C:\Users\A B\Morph\lo")),
            "file:///C:/Users/A%20B/Morph/lo"
        );
        assert_eq!(
            file_url(Path::new(r"C:\Fotos\Férias")),
            "file:///C:/Fotos/F%C3%A9rias"
        );
        assert_eq!(file_url(Path::new(r"\\?\C:\x")), "file:///C:/x");
    }

    #[test]
    fn profiles_are_seeded_with_safe_settings() {
        let dir = tempfile::tempdir().unwrap();
        let xcu = dir.path().join("user").join("registrymodifications.xcu");
        seed_profile(dir.path()).unwrap();
        let text = std::fs::read_to_string(&xcu).unwrap();
        assert!(text.starts_with("<?xml") && text.trim_end().ends_with("</oor:items>"));
        for (p, n, v) in PROFILE_SETTINGS {
            assert_eq!(text.matches(&profile_item(p, n, v)).count(), 1, "{n}");
        }
        assert!(text.contains(r#"oor:name="MacroSecurityLevel" oor:op="fuse"><value>3<"#));
        assert!(text.contains(r#"oor:name="DisableMacrosExecution" oor:op="fuse"><value>true<"#));
        assert!(text.contains(&profile_item(
            "/org.openoffice.Office.Writer/Content/Update",
            "Link",
            "2"
        )));
        assert!(text.contains(&profile_item(
            "/org.openoffice.Office.Calc/Content/Update",
            "Link",
            "1"
        )));
        // Idempotent.
        seed_profile(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&xcu).unwrap(), text);
        // A profile LibreOffice already wrote (older, weaker value) gets ours appended.
        let old = "<?xml version=\"1.0\"?>\n<oor:items>\n<item oor:path=\"/x\"><prop oor:name=\"Keep\" oor:op=\"fuse\"><value>1</value></prop></item>\n\
<item oor:path=\"/org.openoffice.Office.Common/Security/Scripting\"><prop oor:name=\"MacroSecurityLevel\" oor:op=\"fuse\"><value>0</value></prop></item>\n</oor:items>\n";
        std::fs::write(&xcu, old).unwrap();
        seed_profile(dir.path()).unwrap();
        let text = std::fs::read_to_string(&xcu).unwrap();
        assert!(text.contains("\"Keep\""));
        let weak = text.find("<value>0</value></prop></item>").unwrap();
        let strong = text
            .find("MacroSecurityLevel\" oor:op=\"fuse\"><value>3")
            .unwrap();
        assert!(strong > weak, "ours comes later and wins");
        assert!(text.trim_end().ends_with("</oor:items>"));
    }

    #[test]
    fn failures_get_plain_messages() {
        let e = convert_failed("Error: source file could not be loaded");
        assert_eq!(e.kind, ErrorKind::BadInput);
        let e = convert_failed("something else");
        assert_eq!(e.message, "LibreOffice couldn't convert this file.");
    }
}
