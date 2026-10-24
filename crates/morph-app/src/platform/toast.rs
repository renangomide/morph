// SPDX-License-Identifier: GPL-3.0-or-later
//! Toast notification when a job finishes (SPEC §3.3).
//!
//! Needs package identity (MSIX / loose registration). Unpackaged runs return
//! an error, which callers ignore.
//!
//! # Activation
//!
//! The toast body and its "Open folder" button use **protocol activation**:
//! clicking them launches a `morph:` URI, which the manifest maps to
//! `Morph.exe "<uri>"` (`uap3:Protocol Name="morph" Parameters="&quot;%1&quot;"`).
//! This works whether or not Morph is still running and needs no COM activator.
//! [`args_from_uri`] turns the URI back into the usual
//! `--open-folder <path>` / `--show-results <id>` arguments.
//!
//! URIs: `morph:open-folder?path=<percent-encoded UTF-8>` and
//! `morph:show-results?id=<percent-encoded>`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use windows::Data::Xml::Dom::{XmlDocument, XmlElement};
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager,
};
use windows::core::HSTRING;

use crate::strings::{TOAST_OPEN_FOLDER, TOAST_VIEW_RESULTS};

/// URI scheme registered in the manifest (`uap3:Protocol Name`). Keep in sync.
pub const PROTOCOL_SCHEME: &str = "morph";

const ACTION_OPEN_FOLDER: &str = "open-folder";
const ACTION_SHOW_RESULTS: &str = "show-results";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// First line, e.g. "Converted 11 files · saved 38.2 MB" or "11 converted, 1 failed".
    pub title: String,
    /// Optional second line.
    pub body: Option<String>,
    /// Folder for the "Open folder" button (relaunches `Morph.exe --open-folder <path>`).
    pub open_folder: Option<PathBuf>,
    /// Saved results id: clicking the toast relaunches `Morph.exe --show-results <id>`.
    pub results_id: Option<String>,
}

/// Show the toast. Must not block for long; never panics.
///
/// `Show()` succeeds silently even when Windows drops the toast, so the
/// notifier's setting is checked first: an error names why nothing appears
/// (e.g. notifications turned off for the whole system).
pub fn show(toast: &Toast) -> anyhow::Result<()> {
    if morph_formats::paths::current_package_family_name().is_none() {
        bail!("toasts need package identity; running unpackaged");
    }
    let aumid = morph_formats::paths::app_user_model_id().context("no AppUserModelID")?;
    let doc = build_xml(toast).map_err(|e| win_err("could not build the toast XML", &e))?;
    let notification = ToastNotification::CreateToastNotification(&doc)
        .map_err(|e| win_err("could not create the toast", &e))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(&aumid))
        .map_err(|e| win_err("could not create the toast notifier", &e))?;
    match notifier.Setting() {
        Ok(NotificationSetting::Enabled) => {}
        Ok(setting) => bail!(
            "Windows has toasts turned off for {aumid}: {}",
            setting_name(setting)
        ),
        Err(e) => return Err(win_err("could not read the notification setting", &e)),
    }
    notifier
        .Show(&notification)
        .map_err(|e| win_err("could not show the toast", &e))?;
    Ok(())
}

fn win_err(context: &str, e: &windows::core::Error) -> anyhow::Error {
    anyhow::anyhow!("{context}: HRESULT {:#010X}: {}", e.code().0, e.message())
}

fn setting_name(s: NotificationSetting) -> &'static str {
    match s {
        NotificationSetting::DisabledForApplication => "disabled for this app (Settings)",
        NotificationSetting::DisabledForUser => {
            "disabled for the user (notifications off system-wide, \
             e.g. HKLM PushNotifications ToastEnabled=0 or WpnService disabled)"
        }
        NotificationSetting::DisabledByGroupPolicy => "disabled by group policy",
        NotificationSetting::DisabledByManifest => "disabled by the app manifest",
        _ => "not enabled",
    }
}

/// The toast as an XML DOM. The DOM escapes all text and attribute values.
///
/// ```xml
/// <toast launch="morph:show-results?id=…" activationType="protocol">
///   <visual><binding template="ToastGeneric"><text>title</text><text>body</text></binding></visual>
///   <actions>
///     <action content="Open folder" activationType="protocol" arguments="morph:open-folder?path=…"/>
///     <action content="View results" activationType="protocol" arguments="morph:show-results?id=…"/>
///   </actions>
/// </toast>
/// ```
fn build_xml(toast: &Toast) -> windows::core::Result<XmlDocument> {
    let doc = XmlDocument::new()?;
    let root = element(&doc, "toast")?;
    if let Some(id) = &toast.results_id {
        root.SetAttribute(&h("launch"), &HSTRING::from(show_results_uri(id)))?;
        root.SetAttribute(&h("activationType"), &h("protocol"))?;
    }
    doc.AppendChild(&root)?;

    let visual = element(&doc, "visual")?;
    let binding = element(&doc, "binding")?;
    binding.SetAttribute(&h("template"), &h("ToastGeneric"))?;
    for line in std::iter::once(&toast.title).chain(toast.body.as_ref()) {
        let text = element(&doc, "text")?;
        text.AppendChild(&doc.CreateTextNode(&HSTRING::from(line.as_str()))?)?;
        binding.AppendChild(&text)?;
    }
    visual.AppendChild(&binding)?;
    root.AppendChild(&visual)?;

    // Buttons (prototype): Open folder, View results (same as a toast click).
    let buttons: Vec<(&str, String)> = toast
        .open_folder
        .as_deref()
        .map(|folder| (TOAST_OPEN_FOLDER, open_folder_uri(folder)))
        .into_iter()
        .chain(
            toast
                .results_id
                .as_deref()
                .map(|id| (TOAST_VIEW_RESULTS, show_results_uri(id))),
        )
        .collect();
    if !buttons.is_empty() {
        let actions = element(&doc, "actions")?;
        for (content, uri) in buttons {
            let action = element(&doc, "action")?;
            action.SetAttribute(&h("content"), &h(content))?;
            action.SetAttribute(&h("activationType"), &h("protocol"))?;
            action.SetAttribute(&h("arguments"), &HSTRING::from(uri))?;
            actions.AppendChild(&action)?;
        }
        root.AppendChild(&actions)?;
    }
    Ok(doc)
}

fn element(doc: &XmlDocument, name: &str) -> windows::core::Result<XmlElement> {
    doc.CreateElement(&h(name))
}

fn h(s: &str) -> HSTRING {
    HSTRING::from(s)
}

/// `morph:open-folder?path=<encoded>`.
pub fn open_folder_uri(folder: &Path) -> String {
    // Paths that are not valid Unicode (unpaired surrogates) are lossy here;
    // Windows file names in practice are valid.
    format!(
        "{PROTOCOL_SCHEME}:{ACTION_OPEN_FOLDER}?path={}",
        percent_encode(&folder.to_string_lossy())
    )
}

/// `morph:show-results?id=<encoded>`.
pub fn show_results_uri(id: &str) -> String {
    format!(
        "{PROTOCOL_SCHEME}:{ACTION_SHOW_RESULTS}?id={}",
        percent_encode(id)
    )
}

/// If `arg` is a `morph:` URI (toast activation), the equivalent command-line
/// arguments: `[--open-folder, <path>]` or `[--show-results, <id>]`.
/// `None` for anything else, including malformed or unknown `morph:` URIs.
///
/// Meant for `launch::parse`: when the first argument is a `morph:` URI,
/// parse these arguments instead.
pub fn args_from_uri(arg: &OsStr) -> Option<Vec<OsString>> {
    let s = arg.to_str()?.trim();
    let (scheme, rest) = s.split_once(':')?;
    if !scheme.eq_ignore_ascii_case(PROTOCOL_SCHEME) {
        return None;
    }
    // Tolerate `morph://open-folder/?path=…` in case something normalizes the URI.
    let rest = rest.trim_start_matches('/');
    let (action, query) = rest.split_once('?')?;
    let action = action.trim_end_matches('/');
    let (flag, key) = if action.eq_ignore_ascii_case(ACTION_OPEN_FOLDER) {
        (morph_formats::ARG_OPEN_FOLDER, "path")
    } else if action.eq_ignore_ascii_case(ACTION_SHOW_RESULTS) {
        (morph_formats::ARG_SHOW_RESULTS, "id")
    } else {
        return None;
    };
    let value = query
        .split('&')
        .find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key))
        .map(|(_, v)| v)?;
    let value = percent_decode(value)?;
    if value.is_empty() {
        return None;
    }
    Some(vec![OsString::from(flag), OsString::from(value)])
}

/// Percent-encode everything except RFC 3986 unreserved characters, as UTF-8.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode `%XX` sequences (UTF-8). Characters that arrive already decoded
/// (e.g. raw non-ASCII) pass through. `+` is not a space. `None` on bad
/// escapes or invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml(t: &Toast) -> String {
        build_xml(t).unwrap().GetXml().unwrap().to_string()
    }

    #[test]
    fn xml_escapes_text_and_attributes() {
        let t = Toast {
            title: r#"Converted <11> files & "more" · saved 38.2 MB"#.into(),
            body: Some("a</text><text>b".into()),
            open_folder: Some(PathBuf::from(r#"C:\A & B\<x>"#)),
            results_id: Some("abc-123".into()),
        };
        let x = xml(&t);
        assert!(
            x.contains("Converted &lt;11&gt; files &amp; \"more\" · saved 38.2 MB"),
            "{x}"
        );
        assert!(x.contains("a&lt;/text&gt;&lt;text&gt;b"), "{x}");
        assert!(
            x.contains(r#"launch="morph:show-results?id=abc-123""#),
            "{x}"
        );
        assert!(x.contains(r#"activationType="protocol""#), "{x}");
        assert!(x.contains(r#"content="Open folder""#), "{x}");
        assert!(x.contains(r#"content="View results""#), "{x}");
        assert!(
            x.find("Open folder") < x.find("View results"),
            "Open folder first: {x}"
        );
        assert!(
            x.contains("arguments=\"morph:open-folder?path=C%3A%5CA%20%26%20B%5C%3Cx%3E\""),
            "{x}"
        );
        assert!(x.contains(r#"<binding template="ToastGeneric">"#), "{x}");
        // Round-trips through a real XML parser.
        let doc = XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(x)).unwrap();
        assert_eq!(
            doc.GetElementsByTagName(&h("text"))
                .unwrap()
                .Length()
                .unwrap(),
            2
        );
    }

    #[test]
    fn xml_minimal() {
        let t = Toast {
            title: "Done".into(),
            body: None,
            open_folder: None,
            results_id: None,
        };
        let x = xml(&t);
        assert!(!x.contains("launch="), "{x}");
        assert!(!x.contains("<actions"), "{x}");
        assert_eq!(x.matches("<text>").count(), 1, "{x}");
    }

    #[test]
    fn uri_round_trip_unicode_and_spaces() {
        let folder = PathBuf::from(r"D:\Fotos\Férias 夏 100%+#&?=😀");
        let uri = open_folder_uri(&folder);
        assert!(uri.is_ascii() && !uri.contains(' ') && !uri.contains('"'));
        assert_eq!(
            args_from_uri(OsStr::new(&uri)).unwrap(),
            vec![OsString::from("--open-folder"), folder.into_os_string()]
        );
        let uri = show_results_uri("3f2a-bc");
        assert_eq!(
            args_from_uri(OsStr::new(&uri)).unwrap(),
            vec![OsString::from("--show-results"), OsString::from("3f2a-bc")]
        );
    }

    #[test]
    fn uri_tolerates_normalization() {
        // Already-decoded characters, upper-case scheme, `//` and trailing slash.
        assert_eq!(
            args_from_uri(OsStr::new("MORPH://open-folder/?path=D:%5CF%C3%A9rias 夏")).unwrap(),
            vec![
                OsString::from("--open-folder"),
                OsString::from(r"D:\Férias 夏")
            ]
        );
        assert_eq!(
            args_from_uri(OsStr::new("morph:show-results?x=1&id=ab")).unwrap()[1],
            OsString::from("ab")
        );
    }

    #[test]
    fn uri_rejects_garbage() {
        for bad in [
            "",
            r"C:\a.jpg",
            "--open-folder",
            "http://x/open-folder?path=C%3A",
            "morph:",
            "morph:open-folder",
            "morph:open-folder?path=",
            "morph:open-folder?path=%ZZ",
            "morph:open-folder?path=%C3",
            "morph:open-folder?folder=C%3A",
            "morph:delete?path=C%3A",
        ] {
            assert_eq!(args_from_uri(OsStr::new(bad)), None, "{bad}");
        }
    }

    #[test]
    fn unpackaged_show_is_an_error() {
        if morph_formats::paths::current_package_family_name().is_none() {
            let t = Toast {
                title: "x".into(),
                body: None,
                open_folder: None,
                results_id: None,
            };
            assert!(show(&t).is_err());
        }
    }
}
