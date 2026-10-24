// SPDX-License-Identifier: GPL-3.0-or-later
//! Microsoft Office automation over COM late binding (`IDispatch`), SPEC §7, §13.
//!
//! Use [`run_office`]: it runs your closure on a **dedicated STA thread** with
//! a fresh automation instance and handles the dangerous parts:
//!
//! - `CoInitializeEx(STA)`, `CoCreateInstance(<App>.Application, LOCAL_SERVER)`.
//! - `Visible = false` (Word, Excel; PowerPoint can't hide its app window, so
//!   open presentations with `WithWindow := false`), alerts off, macros disabled
//!   (`AutomationSecurity = msoAutomationSecurityForceDisable`), Excel:
//!   `AskToUpdateLinks = false`, `EnableEvents = false`, `IgnoreRemoteRequests = true`
//!   (reset before quitting; it is a persistent setting).
//! - **Finally** (also after an error or panic in the closure): every document you
//!   registered with [`OfficeSession::track`] is closed without saving, then the
//!   app quits, but only if no other document is open in it.
//! - **Watchdog**: the calling thread waits with a timeout and watches the cancel
//!   button. On timeout/cancel it kills the Office process **we started** (found by
//!   comparing `WINWORD.EXE`/`EXCEL.EXE`/`POWERPNT.EXE` process IDs before and
//!   after `CoCreateInstance`), which makes the stuck COM call fail.
//! - One conversion at a time per app, across all Morph processes (named mutex
//!   `Local\Morph-Office-<App>`, so the PID snapshot can't be raced).
//!
//! **Instance decision.** Word and Excel start a new, private process for every
//! `CoCreateInstance`, even when the user has Word/Excel open, so we never touch
//! the user's window and may kill our own instance. PowerPoint is single-instance:
//! if the user already runs it, we get *their* process. Then we never kill it
//! (a timeout just reports an error and abandons the call), restore the alert and
//! macro settings, and quit only when no presentation is left open. If a user
//! opens a document while our hidden Word/Excel runs and Windows routes it into
//! our instance, we don't quit: we make the instance visible and leave it to them.

use super::ENGINE_TIMEOUT;
use super::detect::{AppInfo, OfficeApp};
use super::win;
use crate::converter::TaskCtx;
use crate::error::ConvertError;
use std::cell::RefCell;
use std::fmt;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{DISP_E_EXCEPTION, DISP_E_PARAMNOTFOUND, VARIANT_BOOL};
use windows::Win32::Globalization::LOCALE_USER_DEFAULT;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    CoCreateInstance, CoInitializeEx, CoUninitialize, DISPATCH_FLAGS, DISPATCH_METHOD,
    DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO, IDispatch,
};
use windows::Win32::System::Ole::DISPID_PROPERTYPUT;
use windows::Win32::System::Variant::{
    VAR_CHANGE_FLAGS, VARENUM, VARIANT, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_EMPTY, VT_ERROR, VT_I4,
    VariantChangeType, VariantClear,
};
use windows::core::{BSTR, GUID, HSTRING, PCWSTR};

const POLL: Duration = Duration::from_millis(100);
/// After killing a hung instance, how long we wait for the STA thread to notice.
const KILL_GRACE: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------- errors

/// A failed COM call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComError {
    /// Member that failed, e.g. `Documents.Open`.
    pub member: String,
    /// HRESULT (or the exception's `scode`).
    pub code: i32,
    /// Office's own message, when it gave one ("The password is incorrect…").
    pub description: String,
}

impl fmt::Display for ComError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed: 0x{:08X}", self.member, self.code as u32)?;
        if !self.description.is_empty() {
            write!(f, " ({})", self.description)?;
        }
        Ok(())
    }
}

impl From<ComError> for ConvertError {
    fn from(e: ComError) -> Self {
        ConvertError::other("Microsoft Office couldn't convert this file.", e)
    }
}

// ---------------------------------------------------------------- VARIANT

/// An owned `VARIANT` (cleared on drop).
pub struct Variant(VARIANT);

impl Default for Variant {
    fn default() -> Self {
        Variant::empty()
    }
}

impl Variant {
    pub fn empty() -> Variant {
        Variant(VARIANT::default())
    }

    fn with(vt: VARENUM, set: impl FnOnce(&mut VARIANT)) -> Variant {
        let mut v = VARIANT::default();
        // SAFETY: writing the tag of a zeroed VARIANT.
        unsafe { (*v.Anonymous.Anonymous).vt = vt };
        set(&mut v);
        Variant(v)
    }

    pub fn bool(b: bool) -> Variant {
        Variant::with(VT_BOOL, |v| {
            // SAFETY: tag is VT_BOOL.
            unsafe {
                (*v.Anonymous.Anonymous).Anonymous.boolVal = VARIANT_BOOL(if b { -1 } else { 0 })
            }
        })
    }

    pub fn i32(n: i32) -> Variant {
        Variant::with(VT_I4, |v| {
            // SAFETY: tag is VT_I4.
            unsafe { (*v.Anonymous.Anonymous).Anonymous.lVal = n }
        })
    }

    pub fn str(s: &str) -> Variant {
        Variant::with(VT_BSTR, |v| {
            // SAFETY: tag is VT_BSTR; the BSTR is freed by VariantClear.
            unsafe { (*v.Anonymous.Anonymous).Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(s)) }
        })
    }

    /// A Windows path as a BSTR (lossless for any Unicode name).
    pub fn path(p: &std::path::Path) -> Variant {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = p.as_os_str().encode_wide().collect();
        Variant::with(VT_BSTR, |v| {
            let bstr = BSTR::from_wide(&wide);
            // SAFETY: tag is VT_BSTR; the BSTR is freed by VariantClear.
            unsafe { (*v.Anonymous.Anonymous).Anonymous.bstrVal = ManuallyDrop::new(bstr) }
        })
    }

    /// "Parameter not given" for optional positional arguments.
    pub fn missing() -> Variant {
        Variant::with(VT_ERROR, |v| {
            // SAFETY: tag is VT_ERROR.
            unsafe { (*v.Anonymous.Anonymous).Anonymous.scode = DISP_E_PARAMNOTFOUND.0 }
        })
    }

    pub fn vt(&self) -> VARENUM {
        // SAFETY: the tag is always initialized.
        unsafe { self.0.Anonymous.Anonymous.vt }
    }

    pub fn is_empty(&self) -> bool {
        self.vt() == VT_EMPTY
    }

    pub fn as_dispatch(&self) -> Option<Dispatch> {
        if self.vt() != VT_DISPATCH {
            return None;
        }
        // SAFETY: tag is VT_DISPATCH; cloning adds a reference.
        let d = unsafe { (*self.0.Anonymous.Anonymous.Anonymous.pdispVal).clone() };
        d.map(Dispatch)
    }

    fn changed(&self, vt: VARENUM) -> Option<Variant> {
        let mut out = Variant::empty();
        // SAFETY: both VARIANTs are valid; `out` owns the result.
        unsafe { VariantChangeType(&mut out.0, &self.0, VAR_CHANGE_FLAGS(0), vt) }.ok()?;
        Some(out)
    }

    pub fn to_i32(&self) -> Option<i32> {
        let v = self.changed(VT_I4)?;
        // SAFETY: tag is VT_I4.
        Some(unsafe { v.0.Anonymous.Anonymous.Anonymous.lVal })
    }

    pub fn to_bool(&self) -> Option<bool> {
        let v = self.changed(VT_BOOL)?;
        // SAFETY: tag is VT_BOOL.
        Some(unsafe { v.0.Anonymous.Anonymous.Anonymous.boolVal.0 } != 0)
    }

    pub fn to_string_lossy(&self) -> Option<String> {
        let v = self.changed(VT_BSTR)?;
        // SAFETY: tag is VT_BSTR.
        Some(unsafe { v.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string() })
    }
}

impl Drop for Variant {
    fn drop(&mut self) {
        // SAFETY: frees whatever the VARIANT owns (BSTR, interface).
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

impl fmt::Debug for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Variant(vt={}, {:?})",
            self.vt().0,
            self.to_string_lossy()
        )
    }
}

// ---------------------------------------------------------------- IDispatch

/// A late-bound COM object (`Application`, `Documents`, a document…).
#[derive(Clone, Debug)]
pub struct Dispatch(pub IDispatch);

impl Dispatch {
    fn ids(&self, names: &[&str]) -> Result<Vec<i32>, ComError> {
        let wide: Vec<HSTRING> = names.iter().map(|n| HSTRING::from(*n)).collect();
        let ptrs: Vec<PCWSTR> = wide.iter().map(|h| PCWSTR(h.as_ptr())).collect();
        let mut ids = vec![0i32; names.len()];
        // SAFETY: arrays of `names.len()` entries.
        unsafe {
            self.0.GetIDsOfNames(
                &GUID::zeroed(),
                ptrs.as_ptr(),
                ptrs.len() as u32,
                LOCALE_USER_DEFAULT,
                ids.as_mut_ptr(),
            )
        }
        .map_err(|e| ComError {
            member: names.join("/"),
            code: e.code().0,
            description: "unknown name".into(),
        })?;
        Ok(ids)
    }

    fn invoke(
        &self,
        name: &str,
        flags: DISPATCH_FLAGS,
        positional: Vec<Variant>,
        named: Vec<(&str, Variant)>,
        put: bool,
    ) -> Result<Variant, ComError> {
        let mut names = vec![name];
        names.extend(named.iter().map(|(n, _)| *n));
        let ids = self.ids(&names)?;
        let member = ids[0];
        // rgvarg: named args first (matching rgdispidNamedArgs), then positional in reverse.
        let mut named_ids: Vec<i32> = ids[1..].to_vec();
        let mut args: Vec<VARIANT> = Vec::with_capacity(named.len() + positional.len());
        let mut keep: Vec<Variant> = Vec::new();
        for (_, v) in named {
            keep.push(v);
        }
        for v in positional.into_iter().rev() {
            keep.push(v);
        }
        // Shallow copies; `keep` owns (and frees) the contents.
        for v in &keep {
            // SAFETY: bitwise copy of a VARIANT that `keep` keeps alive.
            args.push(unsafe { std::ptr::read(&v.0) });
        }
        if put {
            // The single positional argument is the new value.
            named_ids = vec![DISPID_PROPERTYPUT];
        }
        let params = DISPPARAMS {
            rgvarg: if args.is_empty() {
                std::ptr::null_mut()
            } else {
                args.as_mut_ptr()
            },
            rgdispidNamedArgs: if named_ids.is_empty() {
                std::ptr::null_mut()
            } else {
                named_ids.as_mut_ptr()
            },
            cArgs: args.len() as u32,
            cNamedArgs: named_ids.len() as u32,
        };
        let mut result = Variant::empty();
        let mut excep = EXCEPINFO::default();
        let mut arg_err = 0u32;
        // SAFETY: all pointers are valid for the call.
        let r = unsafe {
            self.0.Invoke(
                member,
                &GUID::zeroed(),
                LOCALE_USER_DEFAULT,
                flags,
                &params,
                Some(&mut result.0),
                Some(&mut excep),
                Some(&mut arg_err),
            )
        };
        // `args` are shallow copies: forget them, `keep` frees the real ones.
        for a in args {
            std::mem::forget(a);
        }
        drop(keep);
        // SAFETY: take ownership of the exception strings so they are freed.
        let (source, description) = unsafe {
            (
                ManuallyDrop::take(&mut excep.bstrSource),
                ManuallyDrop::take(&mut excep.bstrDescription),
            )
        };
        // SAFETY: as above.
        drop(unsafe { ManuallyDrop::take(&mut excep.bstrHelpFile) });
        match r {
            Ok(()) => Ok(result),
            Err(e) => {
                let code = if e.code() == DISP_E_EXCEPTION && excep.scode != 0 {
                    excep.scode
                } else {
                    e.code().0
                };
                let mut text = description.to_string();
                if text.is_empty() {
                    text = e.message();
                }
                let _ = source;
                Err(ComError {
                    member: name.to_owned(),
                    code,
                    description: text.trim().to_owned(),
                })
            }
        }
    }

    /// Property get: `obj.Name`.
    pub fn get(&self, name: &str) -> Result<Variant, ComError> {
        self.invoke(name, DISPATCH_PROPERTYGET, Vec::new(), Vec::new(), false)
    }

    /// Indexed property get / collection item: `obj.Name(args…)`.
    pub fn get_with(&self, name: &str, args: Vec<Variant>) -> Result<Variant, ComError> {
        self.invoke(
            name,
            DISPATCH_PROPERTYGET | DISPATCH_METHOD,
            args,
            Vec::new(),
            false,
        )
    }

    /// Property get that must return an object (`app.Documents`).
    pub fn object(&self, name: &str) -> Result<Dispatch, ComError> {
        let v = self.get(name)?;
        v.as_dispatch().ok_or_else(|| ComError {
            member: name.to_owned(),
            code: 0,
            description: "not an object".into(),
        })
    }

    /// Property put: `obj.Name = value`.
    pub fn put(&self, name: &str, value: Variant) -> Result<(), ComError> {
        self.invoke(name, DISPATCH_PROPERTYPUT, vec![value], Vec::new(), true)
            .map(|_| ())
    }

    /// Method call with positional arguments.
    pub fn call(&self, name: &str, args: Vec<Variant>) -> Result<Variant, ComError> {
        self.invoke(name, DISPATCH_METHOD, args, Vec::new(), false)
    }

    /// Method call with named arguments: `Documents.Open(FileName:=…, ReadOnly:=True)`.
    pub fn call_named(&self, name: &str, args: Vec<(&str, Variant)>) -> Result<Variant, ComError> {
        self.invoke(name, DISPATCH_METHOD, Vec::new(), args, false)
    }
}

// ---------------------------------------------------------------- session

/// An automation instance, handed to the closure of [`run_office`].
pub struct OfficeSession {
    /// The `Application` object.
    pub app: Dispatch,
    pub which: OfficeApp,
    /// We started this process (not the user's PowerPoint).
    pub owned: bool,
    tracked: RefCell<Vec<Dispatch>>,
    /// The user's own instance: their `AutomationSecurity` / `DisplayAlerts`,
    /// put back when we're done.
    user_settings: Option<UserSettings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UserSettings {
    automation_security: Option<i32>,
    display_alerts: Option<i32>,
}

impl UserSettings {
    fn read(app: &Dispatch) -> UserSettings {
        UserSettings {
            automation_security: app.get("AutomationSecurity").ok().and_then(|v| v.to_i32()),
            display_alerts: app.get("DisplayAlerts").ok().and_then(|v| v.to_i32()),
        }
    }

    /// What to write back: their values, else Office's defaults
    /// (macros by UI, PowerPoint `ppAlertsAll`).
    fn restore_values(self) -> (i32, i32) {
        (
            self.automation_security.unwrap_or(MACROS_BY_UI),
            self.display_alerts.unwrap_or(2),
        )
    }
}

impl OfficeSession {
    /// Register a document/workbook/presentation you opened: it is closed without
    /// saving when the session ends, whatever happens.
    pub fn track(&self, doc: &Dispatch) {
        self.tracked.borrow_mut().push(doc.clone());
    }
}

// msoAutomationSecurityForceDisable / msoAutomationSecurityByUI
const MACROS_OFF: i32 = 3;
const MACROS_BY_UI: i32 = 2;

fn configure(app: &Dispatch, which: OfficeApp, owned: bool) {
    // Best effort: a setting that fails must not stop the conversion.
    let _ = app.put("AutomationSecurity", Variant::i32(MACROS_OFF));
    match which {
        OfficeApp::Word => {
            let _ = app.put("Visible", Variant::bool(false));
            let _ = app.put("DisplayAlerts", Variant::i32(0)); // wdAlertsNone
            let _ = app.put("ScreenUpdating", Variant::bool(false));
        }
        OfficeApp::Excel => {
            let _ = app.put("Visible", Variant::bool(false));
            let _ = app.put("DisplayAlerts", Variant::bool(false));
            let _ = app.put("AskToUpdateLinks", Variant::bool(false));
            let _ = app.put("EnableEvents", Variant::bool(false));
            if owned {
                let _ = app.put("IgnoreRemoteRequests", Variant::bool(true));
            }
        }
        OfficeApp::PowerPoint => {
            let _ = app.put("DisplayAlerts", Variant::i32(1)); // ppAlertsNone
        }
    }
}

fn open_count(app: &Dispatch, which: OfficeApp) -> Option<i32> {
    let collection = match which {
        OfficeApp::Word => "Documents",
        OfficeApp::Excel => "Workbooks",
        OfficeApp::PowerPoint => "Presentations",
    };
    app.object(collection).ok()?.get("Count").ok()?.to_i32()
}

fn close_doc(doc: &Dispatch, which: OfficeApp) {
    let _ = match which {
        OfficeApp::Word => doc.call_named("Close", vec![("SaveChanges", Variant::i32(0))]),
        OfficeApp::Excel => doc.call_named("Close", vec![("SaveChanges", Variant::bool(false))]),
        OfficeApp::PowerPoint => doc.call("Close", Vec::new()),
    };
}

fn finish(session: &OfficeSession) {
    let (app, which) = (&session.app, session.which);
    for doc in session.tracked.borrow_mut().drain(..).rev() {
        close_doc(&doc, which);
    }
    if which == OfficeApp::Excel && session.owned {
        let _ = app.put("IgnoreRemoteRequests", Variant::bool(false));
    }
    match open_count(app, which) {
        Some(0) => {
            let _ = match which {
                OfficeApp::Word => app.call_named("Quit", vec![("SaveChanges", Variant::i32(0))]),
                _ => app.call("Quit", Vec::new()),
            };
        }
        _ if session.owned => {
            // The user opened something in our hidden instance: hand it over.
            let _ = app.put("AutomationSecurity", Variant::i32(MACROS_BY_UI));
            let _ = match which {
                OfficeApp::Word => app.put("DisplayAlerts", Variant::i32(-1)), // wdAlertsAll
                OfficeApp::Excel => app.put("DisplayAlerts", Variant::bool(true)),
                OfficeApp::PowerPoint => app.put("DisplayAlerts", Variant::i32(2)), // ppAlertsAll
            };
            let _ = app.put("ScreenUpdating", Variant::bool(true));
            let _ = app.put("Visible", Variant::bool(true));
        }
        _ => {
            // The user's own PowerPoint: put their settings back, leave it running.
            let (security, alerts) = session
                .user_settings
                .unwrap_or(UserSettings {
                    automation_security: None,
                    display_alerts: None,
                })
                .restore_values();
            let _ = app.put("AutomationSecurity", Variant::i32(security));
            let _ = app.put("DisplayAlerts", Variant::i32(alerts));
        }
    }
}

// ---------------------------------------------------------------- runner

/// `Local\Morph-Office-Word` etc.: one conversion per Office app across every
/// Morph process of the session, so two `Morph.exe` never race the PID
/// snapshot that tells which instance is ours.
pub fn app_lock_name(which: OfficeApp) -> String {
    format!("Local\\Morph-Office-{which:?}")
}

fn app_lock(
    which: OfficeApp,
    ctx: &TaskCtx<'_>,
    deadline: Instant,
) -> Result<win::NamedMutex, ConvertError> {
    let name = app_lock_name(which);
    loop {
        if let Some(lock) = win::NamedMutex::try_acquire(&name) {
            return Ok(lock);
        }
        ctx.check_cancel()?;
        if Instant::now() >= deadline {
            return Err(ConvertError::other(
                format!("{} is busy. Try again in a moment.", which.display_name()),
                "automation lock timeout",
            ));
        }
        std::thread::sleep(POLL);
    }
}

type Outcome<T> = Result<T, ConvertError>;

/// Run `work` against a fresh automation instance of `info.app` on its own STA
/// thread, with the default 180 s timeout. See the module docs.
pub fn run_office<T, F>(info: &AppInfo, ctx: &TaskCtx<'_>, work: F) -> Outcome<T>
where
    T: Send + 'static,
    F: FnOnce(&OfficeSession) -> Outcome<T> + Send + 'static,
{
    run_office_with_timeout(info, ctx, ENGINE_TIMEOUT, work)
}

/// [`run_office`] with an explicit timeout.
pub fn run_office_with_timeout<T, F>(
    info: &AppInfo,
    ctx: &TaskCtx<'_>,
    timeout: Duration,
    work: F,
) -> Outcome<T>
where
    T: Send + 'static,
    F: FnOnce(&OfficeSession) -> Outcome<T> + Send + 'static,
{
    ctx.check_cancel()?;
    let which = info.app;
    let deadline = Instant::now() + timeout;
    let _guard = app_lock(which, ctx, deadline)?;

    let owned_pid: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
    // Set when we give up (timeout/cancel). If the instance's PID is only known
    // after that, the automation thread kills its own instance itself.
    let abandoned = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Outcome<T>>();
    let pid_slot = Arc::clone(&owned_pid);
    let gave_up = Arc::clone(&abandoned);
    let spawned = std::thread::Builder::new()
        .name(format!("morph-office-{which:?}"))
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                sta_main(which, &pid_slot, &gave_up, work)
            }))
            .unwrap_or_else(|_| {
                Err(ConvertError::other(
                    "Something went wrong while converting this file.",
                    "panic in the Office automation thread",
                ))
            });
            let _ = tx.send(result);
        });
    if let Err(e) = spawned {
        return Err(ConvertError::other("Couldn't start Microsoft Office.", e));
    }

    loop {
        match rx.recv_timeout(POLL) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(ConvertError::other(
                    "Something went wrong while converting this file.",
                    "Office automation thread ended without a result",
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let cancelled = ctx.cancel.is_cancelled();
        if cancelled || Instant::now() >= deadline {
            abandoned.store(true, Ordering::SeqCst);
            let pid = owned_pid.lock().ok().and_then(|g| *g);
            if let Some(pid) = pid {
                win::kill_pid(pid);
                // Let the STA thread fail out of the dead call and clean up.
                let _ = rx.recv_timeout(KILL_GRACE);
            }
            return Err(if cancelled {
                ConvertError::cancelled()
            } else {
                ConvertError::other(
                    format!("{} took too long and was stopped.", which.display_name()),
                    format!(
                        "timeout after {}s{}",
                        timeout.as_secs(),
                        if pid.is_none() {
                            " (instance not known yet or shared; ours is killed as soon as known)"
                        } else {
                            ""
                        }
                    ),
                )
            });
        }
    }
}

struct ComInit;

impl ComInit {
    fn new() -> Result<ComInit, ConvertError> {
        // SAFETY: initializes COM for this (new, dedicated) thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        hr.ok()
            .map(|()| ComInit)
            .map_err(|e| ConvertError::other("Couldn't start Microsoft Office.", e))
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        // SAFETY: paired with a successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() };
    }
}

fn sta_main<T, F>(
    which: OfficeApp,
    pid_slot: &Mutex<Option<u32>>,
    abandoned: &AtomicBool,
    work: F,
) -> Outcome<T>
where
    F: FnOnce(&OfficeSession) -> Outcome<T>,
{
    let _com = ComInit::new()?;
    let before = win::pids_named(which.exe_name());
    let prog_id = HSTRING::from(which.prog_id());
    let start_failed = |e: windows::core::Error| {
        ConvertError::other(
            format!("{} couldn't be started.", which.display_name()),
            format!("{}: {e}", which.prog_id()),
        )
    };
    // SAFETY: valid ProgID string.
    let clsid = unsafe { CLSIDFromProgID(&prog_id) }.map_err(start_failed)?;
    // SAFETY: valid CLSID; out-of-process server.
    let app: IDispatch =
        unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER) }.map_err(start_failed)?;
    let new: Vec<u32> = win::pids_named(which.exe_name())
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();
    let owned = match new.as_slice() {
        [pid] => Some(*pid),
        _ => None,
    };
    if let Ok(mut slot) = pid_slot.lock() {
        *slot = owned;
    }
    // The watchdog gave up while Office was starting: it couldn't kill an
    // instance it didn't know about yet, so do it here.
    if abandoned.load(Ordering::SeqCst) {
        if let Some(pid) = owned {
            win::kill_pid(pid);
        }
        return Err(ConvertError::other(
            format!("{} took too long and was stopped.", which.display_name()),
            "gave up while the instance was starting",
        ));
    }

    let app = Dispatch(app);
    // Not ours (the user's running PowerPoint): remember their settings first.
    let user_settings = owned.is_none().then(|| UserSettings::read(&app));
    let session = OfficeSession {
        app,
        which,
        owned: owned.is_some(),
        tracked: RefCell::new(Vec::new()),
        user_settings,
    };
    configure(&session.app, which, session.owned);
    let result = catch_unwind(AssertUnwindSafe(|| work(&session))).unwrap_or_else(|_| {
        Err(ConvertError::other(
            "Something went wrong while converting this file.",
            "panic in Office automation code",
        ))
    });
    finish(&session);
    drop(session);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_convert() {
        assert_eq!(Variant::i32(42).to_i32(), Some(42));
        assert_eq!(Variant::bool(true).to_bool(), Some(true));
        assert_eq!(Variant::str("12").to_i32(), Some(12));
        assert_eq!(
            Variant::path(std::path::Path::new(r"C:\Férias 夏.docx"))
                .to_string_lossy()
                .as_deref(),
            Some(r"C:\Férias 夏.docx")
        );
        assert!(Variant::empty().is_empty());
        assert!(Variant::missing().as_dispatch().is_none());
        assert_eq!(Variant::missing().vt(), VT_ERROR);
    }

    #[test]
    fn user_settings_are_restored_not_reset() {
        let theirs = UserSettings {
            automation_security: Some(1), // msoAutomationSecurityLow
            display_alerts: Some(1),      // ppAlertsNone
        };
        assert_eq!(theirs.restore_values(), (1, 1));
        let unknown = UserSettings {
            automation_security: None,
            display_alerts: None,
        };
        assert_eq!(unknown.restore_values(), (MACROS_BY_UI, 2));
    }

    #[test]
    fn office_locks_are_session_wide_per_app() {
        assert_eq!(app_lock_name(OfficeApp::Word), "Local\\Morph-Office-Word");
        assert_ne!(
            app_lock_name(OfficeApp::Excel),
            app_lock_name(OfficeApp::PowerPoint)
        );
    }

    #[test]
    fn com_errors_read_well() {
        let e = ComError {
            member: "Documents.Open".into(),
            code: 0x800A1520u32 as i32,
            description: "The password is incorrect.".into(),
        };
        assert_eq!(
            e.to_string(),
            "Documents.Open failed: 0x800A1520 (The password is incorrect.)"
        );
        let c: ConvertError = e.into();
        assert!(c.details.contains("password"));
    }

    /// Real Word automation: only where Word is installed.
    #[test]
    #[ignore = "needs Microsoft Word (not installed on the dev machine); run with --ignored"]
    fn word_starts_and_quits() {
        let engines = crate::office::detect::engines();
        let Some(word) = engines.word.clone() else {
            println!("SKIPPED: Microsoft Word not detected");
            return;
        };
        let cancel = crate::CancelToken::new();
        let temp = tempfile::tempdir().unwrap();
        let ctx = TaskCtx::new(&cancel, &|_, _| {}, temp.path());
        let version = run_office(&word, &ctx, |s| {
            Ok(s.app.get("Version")?.to_string_lossy().unwrap_or_default())
        })
        .unwrap();
        println!("Word version {version}");
        assert!(!version.is_empty());
    }
}
