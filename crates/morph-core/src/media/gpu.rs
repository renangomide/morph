// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU encoder detection (SPEC §7): a 1-second test encode per encoder
//! (`h264_nvenc`, `h264_qsv`, `h264_amf`, + hevc variants) at first use, cached
//! in `state.toml` ([`crate::state::State::gpu`]) with the FFmpeg version and a
//! driver key (display adapter names + driver versions from the registry).

use super::encode::{Backend, Codec, Encoder};
use super::ffmpeg;
use super::options::EncoderChoice;
use crate::cancel::CancelToken;
use crate::converter::TaskCtx;
use crate::office::win::{self, Hive, View};
use crate::state::State;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The detection result as stored in `state.toml` (`[gpu]`). A different FFmpeg
/// version or driver key means: detect again.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuCache {
    pub ffmpeg_version: String,
    /// Display adapters: `"<DriverDesc> <DriverVersion>"` per adapter, joined with `; `.
    pub driver_key: String,
    /// FFmpeg encoder names whose test encode worked, e.g. `["h264_nvenc", "hevc_nvenc"]`.
    pub working: Vec<String>,
}

/// GPU backends in the order Auto prefers them.
const ORDER: [Backend; 3] = [Backend::Nvenc, Backend::Qsv, Backend::Amf];
/// Time limit for one test encode (a working encoder needs ~1 s; six tests at
/// most 30 s on a machine whose drivers hang).
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Display adapters class key.
const DISPLAY_CLASS: &str =
    r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

/// The encoder to use for `codec`: with `Auto`, the first working GPU encoder
/// (NVENC → QSV → AMF; H.264/H.265 only), else the CPU one.
pub fn pick_encoder(codec: Codec, choice: EncoderChoice, ctx: &TaskCtx<'_>) -> Encoder {
    if choice == EncoderChoice::Cpu || !matches!(codec, Codec::H264 | Codec::H265) {
        return Encoder::cpu(codec);
    }
    let checking = || ctx.status(CHECKING_GPU);
    let detection = detect_with(default_state_path().as_deref(), ctx.cancel, &checking);
    if !detection.from_cache {
        ctx.status("");
    }
    let working = detection.working;
    // A GPU encoder that already failed in this process: the CPU one.
    super::encode::usable(first_working(codec, &working))
}

fn first_working(codec: Codec, working: &[String]) -> Encoder {
    ORDER
        .into_iter()
        .map(|backend| Encoder { codec, backend })
        .find(|e| {
            e.ffmpeg_name()
                .is_some_and(|n| working.iter().any(|w| w == n))
        })
        .unwrap_or(Encoder::cpu(codec))
}

/// Result of [`detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub working: Vec<String>,
    /// `true`: read from `state.toml`, no test encode ran.
    pub from_cache: bool,
}

/// Working GPU encoders: the `state.toml` cache at `state_path` when it matches
/// this FFmpeg + drivers, else test encodes (then saved there). `None` = don't
/// cache. Cancelled midway → nothing cached, CPU only.
pub fn detect(state_path: Option<&Path>, cancel: &CancelToken) -> Detection {
    detect_with(state_path, cancel, &|| {})
}

/// Row status while the first detection runs (up to ~30 s).
pub const CHECKING_GPU: &str = "Checking the graphics card…";

/// [`detect`], calling `on_test` once before the first test encode (not when
/// the cache answers).
pub fn detect_with(
    state_path: Option<&Path>,
    cancel: &CancelToken,
    on_test: &dyn Fn(),
) -> Detection {
    // Two FFmpeg workers starting together detect once.
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(version) = ffmpeg::version() else {
        return Detection {
            working: Vec::new(),
            from_cache: false,
        };
    };
    let key = driver_key();
    if let Some(cached) = state_path.and_then(|p| State::load(p).gpu)
        && cached.ffmpeg_version == version
        && cached.driver_key == key
    {
        return Detection {
            working: cached.working,
            from_cache: true,
        };
    }
    on_test();
    let mut working = Vec::new();
    for codec in [Codec::H264, Codec::H265] {
        for backend in ORDER {
            if cancel.is_cancelled() {
                return Detection {
                    working: Vec::new(),
                    from_cache: false,
                };
            }
            if let Some(name) = (Encoder { codec, backend }).ffmpeg_name()
                && test_encode(name, cancel)
            {
                working.push(name.to_owned());
            }
        }
    }
    if cancel.is_cancelled() {
        return Detection {
            working: Vec::new(),
            from_cache: false,
        };
    }
    if let Some(path) = state_path {
        save(path, version, &key, &working);
    }
    Detection {
        working,
        from_cache: false,
    }
}

fn save(path: &Path, version: &str, key: &str, working: &[String]) {
    let cache = GpuCache {
        ffmpeg_version: version.to_owned(),
        driver_key: key.to_owned(),
        working: working.to_vec(),
    };
    // A cache we can't write only costs a re-detect next time.
    let _ = State::save_gpu(path, cache);
}

/// 1 s of 720p test pattern through `encoder` to the null muxer.
fn test_encode(encoder: &str, cancel: &CancelToken) -> bool {
    let Ok(tools) = ffmpeg::tools() else {
        return false;
    };
    let args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=d=1:s=1280x720",
        "-c:v",
        encoder,
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    ffmpeg::capture(&tools.ffmpeg, &args, cancel, TEST_TIMEOUT).is_ok_and(|out| out.success)
}

/// Display adapter names + driver versions: changes when a GPU or driver changes.
pub fn driver_key() -> String {
    let mut parts: Vec<String> =
        win::reg_subkeys(Hive::LocalMachine, View::Native64, DISPLAY_CLASS)
            .into_iter()
            .filter(|k| k.chars().all(|c| c.is_ascii_digit()))
            .filter_map(|k| {
                let sub = format!(r"{DISPLAY_CLASS}\{k}");
                let read = |v| win::reg_string(Hive::LocalMachine, View::Native64, &sub, Some(v));
                let desc = read("DriverDesc")?;
                Some(format!(
                    "{desc} {}",
                    read("DriverVersion").unwrap_or_default()
                ))
            })
            .collect();
    parts.sort();
    parts.join("; ")
}

/// The app's `state.toml` (where [`pick_encoder`] caches).
pub fn default_state_path() -> Option<PathBuf> {
    morph_formats::paths::app_data_dir().map(|d| morph_formats::paths::state_file(&d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prefers_nvenc_then_qsv_then_amf() {
        let w = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let pick = |c, v: &[&str]| first_working(c, &w(v)).backend;
        assert_eq!(
            pick(Codec::H264, &["h264_amf", "h264_nvenc"]),
            Backend::Nvenc
        );
        assert_eq!(pick(Codec::H264, &["h264_amf", "h264_qsv"]), Backend::Qsv);
        assert_eq!(pick(Codec::H265, &["h264_nvenc", "hevc_amf"]), Backend::Amf);
        assert_eq!(pick(Codec::H265, &["h264_nvenc"]), Backend::Cpu);
        assert_eq!(pick(Codec::H264, &[]), Backend::Cpu);
    }

    #[test]
    fn driver_key_is_stable() {
        assert_eq!(driver_key(), driver_key());
    }
}
