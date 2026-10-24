// SPDX-License-Identifier: GPL-3.0-or-later
//! The main window (SPEC §3.4), shown on a Start menu launch (`page == 3`):
//! **Convert** (drop zone + "click to browse") and **Settings** (all sections,
//! Appearance, About). UI in `ui/shell_view.slint` (`HomePage`, `HomeUi` global).
//!
//! The app hands this module [`HomeHooks`]; everything else (the `HomeUi`
//! callbacks, the file drop, the Settings form) lives here and in files this
//! module adds.

mod form;

use crate::platform::shell;
use crate::{HomeUi, MainWindow, applog, strings};
use form::Form;
use morph_core::Settings;
use morph_core::office::{LIBREOFFICE_DOWNLOAD_URL, detect};
use morph_core::state::State;
use morph_formats::{APP_DISPLAY_NAME, paths};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// What the main window may ask of the app.
pub struct HomeHooks {
    /// Dropped or browsed files: opens the conversion window flow (same
    /// grouping as the right-click menu; unsupported files show as Skipped).
    pub open_files: Box<dyn Fn(Vec<PathBuf>)>,
    /// The current settings (`config.toml`).
    pub settings: Box<dyn Fn() -> Settings>,
    /// Save new settings: written to `config.toml` and used from now on
    /// (job config, defaults, theme, toast, output). `Err`: not written.
    pub save_settings: Box<dyn Fn(Settings) -> io::Result<()>>,
}

/// What the app keeps of the main window after [`wire`].
pub struct HomeHandle(Rc<Home>);

impl HomeHandle {
    /// Run a delayed save (slider) now. Call after the event loop ends: its
    /// timer won't fire any more.
    pub fn flush_pending_save(&self) {
        if self.0.save_timer.running() {
            self.0.save_timer.stop();
            self.0.save();
        }
    }
}

/// `MainWindow::page` of the main window.
const PAGE_HOME: i32 = 3;
/// Sliders and the PNG level save after this pause, not on every step.
const SAVE_DELAY: Duration = Duration::from_millis(400);

struct Home {
    window: Weak<MainWindow>,
    open_files: Box<dyn Fn(Vec<PathBuf>)>,
    settings: Box<dyn Fn() -> Settings>,
    save_settings: Box<dyn Fn(Settings) -> io::Result<()>>,
    save_timer: Timer,
}

/// Wire the main window's callbacks. Called once at startup, whatever the launch.
pub fn wire(window: &MainWindow, hooks: HomeHooks) -> HomeHandle {
    let HomeHooks {
        open_files,
        settings,
        save_settings,
    } = hooks;
    let home = Rc::new(Home {
        window: window.as_weak(),
        open_files,
        settings,
        save_settings,
        save_timer: Timer::default(),
    });

    // Drops count only while the drop zone shows.
    let weak = window.as_weak();
    let hover = window.as_weak();
    let h = home.clone();
    crate::platform::file_drop::on_files_dropped(
        window.window(),
        move || {
            weak.upgrade()
                .is_some_and(|w| w.get_page() == PAGE_HOME && w.global::<HomeUi>().get_nav() == 0)
        },
        move |over| {
            if let Some(w) = hover.upgrade() {
                w.global::<HomeUi>().set_drag_over(over);
            }
        },
        move |files| (h.open_files)(files),
    );

    let ui = window.global::<HomeUi>();
    set_text(&ui);
    set_form(&ui, &Form::from_settings(&(home.settings)()));

    let h = home.clone();
    ui.on_browse_files(move || h.browse_later());
    let h = home.clone();
    ui.on_show_settings(move || h.show_settings(false));
    let h = home.clone();
    ui.on_check_engines(move || h.show_settings(true));
    let h = home.clone();
    ui.on_changed(move || {
        h.save_timer.stop();
        // "Always in this folder" picked before any folder was chosen: ask for one.
        let needs_folder = (h.settings)().output.folder.is_none();
        let wants_folder = h
            .window
            .upgrade()
            .is_some_and(|w| w.global::<HomeUi>().get_output_mode() == 1);
        if needs_folder && wants_folder {
            h.choose_folder_later();
        } else {
            h.save();
        }
    });
    let h = Rc::downgrade(&home);
    ui.on_changed_soon(move || {
        if let Some(home) = h.upgrade() {
            let h = Rc::downgrade(&home);
            home.save_timer
                .start(TimerMode::SingleShot, SAVE_DELAY, move || {
                    if let Some(home) = h.upgrade() {
                        home.save();
                    }
                });
        }
    });
    let h = home.clone();
    ui.on_choose_folder(move || h.choose_folder_later());
    ui.on_download_libreoffice(|| open_url(LIBREOFFICE_DOWNLOAD_URL));
    ui.on_open_github(|| open_url(strings::GITHUB_URL));
    ui.on_open_issues(|| open_url(strings::ISSUES_URL));
    let h = home.clone();
    ui.on_open_logs(move || h.open_logs());
    HomeHandle(home)
}

fn shared(s: &str) -> SharedString {
    SharedString::from(s)
}

fn list<S: AsRef<str>>(items: &[S]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.iter().map(|s| shared(s.as_ref())).collect::<Vec<_>>(),
    ))
}

fn open_url(url: &str) {
    if let Err(err) = shell::open_url(url) {
        applog::error("could not open a link", &err);
    }
}

/// Logical CPU count (at least 1).
fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Last entry of the parallel jobs list (this machine's core count).
fn max_jobs_shown() -> i32 {
    i32::try_from(cores()).unwrap_or(1)
}

fn set_text(ui: &HomeUi) {
    use strings as s;
    ui.set_nav_convert(shared(s::NAV_CONVERT));
    ui.set_nav_settings(shared(s::NAV_SETTINGS));
    ui.set_back_label(shared(s::BACK));
    ui.set_drop_title(shared(s::DROP_TITLE));
    ui.set_nav_tip_title(shared(s::NAV_TIP_TITLE));
    ui.set_nav_tip_text(shared(s::NAV_TIP_TEXT));
    ui.set_drop_hint(shared(&s::drop_hint(APP_DISPLAY_NAME)));
    ui.set_browse(shared(s::BROWSE));
    let cards = s::DROP_CARDS;
    ui.set_drop_card_names(list(&cards.map(|(name, _)| name)));
    ui.set_drop_card_formats(list(&cards.map(|(_, formats)| formats)));
    ui.set_settings_description(shared(s::SETTINGS_DESCRIPTION));
    ui.set_sec_file_types(shared(s::SECTION_FILE_TYPES));
    ui.set_file_types_hint(shared(s::FILE_TYPES_HINT));
    let [images, pdf, video, audio, office] = s::MENU_TYPE_LABELS;
    ui.set_type_images(shared(images));
    ui.set_type_pdf(shared(pdf));
    ui.set_type_video(shared(video));
    ui.set_type_audio(shared(audio));
    ui.set_type_office(shared(office));
    ui.set_type_descriptions(list(&s::TYPE_DESCRIPTIONS));
    ui.set_sec_defaults(shared(s::SECTION_DEFAULTS));
    ui.set_defaults_title(shared(s::DEFAULTS_TITLE));
    ui.set_defaults_hint(shared(s::DEFAULTS_HINT));
    ui.set_jpeg_quality_label(shared(s::JPEG_QUALITY));
    ui.set_webp_quality_label(shared(s::WEBP_QUALITY));
    ui.set_avif_quality_label(shared(s::AVIF_QUALITY));
    ui.set_png_level_label(shared(s::PNG_LEVEL));
    ui.set_pdf_preset_label(shared(s::DEFAULT_PDF_PRESET));
    ui.set_pdf_preset_names(list(&s::DEFAULT_PDF_PRESETS));
    ui.set_video_preset_label(shared(s::DEFAULT_VIDEO_PRESET));
    ui.set_video_preset_names(list(&s::DEFAULT_VIDEO_PRESETS));
    ui.set_video_encoder_label(shared(s::DEFAULT_VIDEO_ENCODER));
    ui.set_video_encoder_names(list(&s::DEFAULT_VIDEO_ENCODERS));
    ui.set_mp3_bitrate_label(shared(s::DEFAULT_MP3_BITRATE));
    ui.set_mp3_bitrate_names(list(&s::DEFAULT_MP3_BITRATES));
    ui.set_pdf_dpi_label(shared(s::DEFAULT_PDF_DPI));
    ui.set_pdf_dpi_names(list(&s::DEFAULT_PDF_DPIS));
    ui.set_sec_output(shared(s::SECTION_OUTPUT));
    ui.set_output_location_label(shared(s::OUTPUT_LOCATION));
    ui.set_output_location_hint(shared(s::OUTPUT_LOCATION_HINT));
    ui.set_output_next_to(shared(s::OUTPUT_NEXT_TO));
    ui.set_output_folder(shared(s::OUTPUT_FOLDER));
    ui.set_choose_folder_label(shared(s::CHOOSE_FOLDER));
    ui.set_open_folder_label(shared(s::OPEN_FOLDER_WHEN_DONE));
    ui.set_keep_dates_label(shared(s::KEEP_FILE_DATES));
    ui.set_sec_engines(shared(s::SECTION_ENGINES));
    ui.set_office_engine_label(shared(s::SECTION_OFFICE));
    ui.set_office_engine_hint(shared(s::OFFICE_ENGINE_HINT));
    ui.set_engine_names(list(&s::ENGINE_CHOICES));
    ui.set_check_again(shared(s::CHECK_ENGINES_AGAIN));
    ui.set_download_libreoffice_label(shared(s::DOWNLOAD_LIBREOFFICE_LINK));
    ui.set_image_jobs_label(shared(s::IMAGE_JOBS));
    ui.set_image_jobs_hint(shared(s::IMAGE_JOBS_HINT));
    let cores = cores();
    let mut jobs = vec![s::image_jobs_auto(cores.saturating_sub(1).max(1))];
    jobs.extend((1..=cores).map(|n| n.to_string()));
    ui.set_image_jobs_names(list(&jobs));
    ui.set_use_gpu_label(shared(s::USE_GPU));
    ui.set_toast_label(shared(s::TOAST));
    ui.set_toast_hint(shared(s::TOAST_HINT));
    ui.set_sec_appearance(shared(s::SECTION_APPEARANCE_NOTIFICATIONS));
    ui.set_theme_label(shared(s::THEME_LABEL));
    ui.set_theme_hint(shared(s::THEME_HINT));
    ui.set_theme_names(list(&s::THEME_NAMES));
    ui.set_sec_about(shared(s::SECTION_ABOUT));
    ui.set_version_text(shared(&s::about_title(
        APP_DISPLAY_NAME,
        &s::version(env!("CARGO_PKG_VERSION")),
    )));
    ui.set_about_description(shared(&s::about_description(APP_DISPLAY_NAME)));
    ui.set_github_label(shared(s::GITHUB));
    ui.set_issues_label(shared(s::REPORT_ISSUE));
    ui.set_open_logs_label(shared(s::OPEN_LOG_FOLDER));
    ui.set_third_party_label(shared(s::THIRD_PARTY));
    ui.set_third_party_summary(shared(s::THIRD_PARTY_SUMMARY));
    ui.set_third_party_hint(shared(&s::third_party_hint(APP_DISPLAY_NAME)));
    let third: Vec<String> = s::THIRD_PARTY_LIST
        .iter()
        .map(|(name, license)| s::third_party_line(name, license))
        .collect();
    ui.set_third_party(list(&third));
}

fn folder_text(folder: Option<&Path>) -> SharedString {
    folder.map_or_else(
        || shared(strings::OUTPUT_NO_FOLDER),
        |f| shared(&f.display().to_string()),
    )
}

fn set_form(ui: &HomeUi, f: &Form) {
    ui.set_images(f.images);
    ui.set_pdf(f.pdf);
    ui.set_video(f.video);
    ui.set_audio(f.audio);
    ui.set_office(f.office);
    ui.set_jpeg_quality(f.jpeg_quality as f32);
    ui.set_webp_quality(f.webp_quality as f32);
    ui.set_avif_quality(f.avif_quality as f32);
    ui.set_defaults_summary(shared(&defaults_summary(f)));
    ui.set_png_level(f.png_level);
    ui.set_pdf_preset(f.pdf_preset);
    ui.set_video_preset(f.video_preset);
    ui.set_video_encoder(f.video_encoder);
    ui.set_mp3_bitrate(f.mp3_bitrate);
    ui.set_pdf_dpi(f.pdf_dpi);
    ui.set_output_mode(f.output_mode);
    ui.set_folder_text(folder_text(f.folder.as_deref()));
    ui.set_open_folder_when_done(f.open_folder_when_done);
    ui.set_keep_dates(f.keep_file_dates);
    ui.set_engine(f.engine);
    // Higher than this machine's core count: show the last entry.
    ui.set_image_jobs(f.image_jobs.min(max_jobs_shown()));
    ui.set_use_gpu(f.use_gpu);
    ui.set_toast(f.toast);
    ui.set_theme(f.theme);
}

/// Subtitle of the Defaults card: "JPG 85 · WebP 80 · … · 150 DPI".
fn defaults_summary(f: &Form) -> String {
    let pick = |names: &[&'static str], i: i32| {
        let last = names.len().saturating_sub(1);
        names
            .get(usize::try_from(i).unwrap_or(0).min(last))
            .copied()
            .unwrap_or_default()
    };
    strings::defaults_summary(
        f.jpeg_quality,
        f.webp_quality,
        f.avif_quality,
        f.png_level,
        pick(&strings::DEFAULT_PDF_PRESETS, f.pdf_preset),
        pick(&strings::DEFAULT_VIDEO_PRESETS, f.video_preset),
        pick(&strings::DEFAULT_MP3_BITRATES, f.mp3_bitrate),
        pick(&strings::DEFAULT_PDF_DPIS, f.pdf_dpi),
    )
}

/// The form as the UI holds it now. `folder` isn't shown in an editable
/// control, so it comes from `base`.
fn read_form(ui: &HomeUi, folder: Option<PathBuf>) -> Form {
    let q = |v: f32| v.round() as i32;
    Form {
        images: ui.get_images(),
        pdf: ui.get_pdf(),
        video: ui.get_video(),
        audio: ui.get_audio(),
        office: ui.get_office(),
        jpeg_quality: q(ui.get_jpeg_quality()),
        webp_quality: q(ui.get_webp_quality()),
        avif_quality: q(ui.get_avif_quality()),
        png_level: ui.get_png_level(),
        pdf_preset: ui.get_pdf_preset(),
        video_preset: ui.get_video_preset(),
        video_encoder: ui.get_video_encoder(),
        mp3_bitrate: ui.get_mp3_bitrate(),
        pdf_dpi: ui.get_pdf_dpi(),
        output_mode: ui.get_output_mode(),
        folder,
        open_folder_when_done: ui.get_open_folder_when_done(),
        keep_file_dates: ui.get_keep_dates(),
        engine: ui.get_engine(),
        image_jobs: ui.get_image_jobs(),
        use_gpu: ui.get_use_gpu(),
        toast: ui.get_toast(),
        theme: ui.get_theme(),
    }
}

/// "Microsoft Word 16.0 found" lines with found flags (LibreOffice last).
fn engine_status(refresh: bool) -> Vec<(String, bool)> {
    use detect::OfficeApp;
    let engines = if refresh {
        detect::refresh()
    } else {
        detect::engines()
    };
    let line = |name: &str, version: Option<String>| {
        let found = version.is_some();
        (strings::engine_line(name, version.as_deref()), found)
    };
    let mut lines: Vec<_> = [OfficeApp::Word, OfficeApp::Excel, OfficeApp::PowerPoint]
        .into_iter()
        .map(|app| {
            let version = engines
                .office_app(app)
                .map(|i| i.short_version().unwrap_or_default());
            line(app.display_name(), version)
        })
        .collect();
    let version = engines
        .libreoffice
        .as_ref()
        .map(|i| i.short_version().unwrap_or_default());
    lines.push(line(strings::LIBREOFFICE, version));
    lines
}

/// The cached GPU detection from `state.toml` (never runs a detection).
fn gpu_status() -> String {
    let gpu = paths::app_data_dir().and_then(|d| State::load(&paths::state_file(&d)).gpu);
    match gpu {
        None => strings::gpu_not_checked(APP_DISPLAY_NAME),
        Some(cache) if cache.working.is_empty() => strings::GPU_NONE.to_owned(),
        Some(cache) => strings::gpu_found(&cache.working.join(", ")),
    }
}

impl Home {
    fn with_ui(&self, f: impl FnOnce(HomeUi<'_>)) {
        if let Some(w) = self.window.upgrade() {
            f(w.global::<HomeUi>());
        }
    }

    /// Settings page opened (or "Check again"): current values + engine status.
    fn show_settings(&self, refresh_engines: bool) {
        let settings = (self.settings)();
        let lines = engine_status(refresh_engines);
        let lo_missing = lines.last().is_some_and(|(_, found)| !found);
        let text: Vec<&str> = lines.iter().map(|(text, _)| text.as_str()).collect();
        let found: Vec<bool> = lines.iter().map(|(_, found)| *found).collect();
        let gpu = gpu_status();
        self.with_ui(|ui| {
            set_form(&ui, &Form::from_settings(&settings));
            ui.set_engine_status(list(&text));
            ui.set_engine_found(ModelRc::new(VecModel::from(found)));
            ui.set_libreoffice_missing(lo_missing);
            ui.set_gpu_status(shared(&gpu));
        });
    }

    /// Save the page's values now; show a plain error if `config.toml` wasn't written.
    fn save(&self) {
        self.save_with_folder(None);
    }

    fn save_with_folder(&self, picked: Option<PathBuf>) {
        let base = (self.settings)();
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let ui = w.global::<HomeUi>();
        let folder = picked.or_else(|| base.output.folder.clone());
        let mut form = read_form(&ui, folder);
        form.keep_unshown_jobs(&base, max_jobs_shown());
        let new = form.apply(&base);
        // "Always in this folder" without a folder: back to "next to originals".
        set_form(&ui, &Form::from_settings(&new));
        ui.set_save_error(match (self.save_settings)(new) {
            Ok(()) => SharedString::new(),
            Err(_) => shared(strings::SAVE_FAILED),
        });
    }

    /// Multi-select file picker → the conversion window. Runs after this
    /// callback returns: the dialog has its own message loop.
    fn browse_later(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        Timer::single_shot(Duration::ZERO, move || {
            let Some(me) = me.upgrade() else {
                return;
            };
            let mut dialog = rfd::FileDialog::new().set_title(strings::BROWSE_TITLE);
            if let Some(w) = me.window.upgrade() {
                dialog = dialog.set_parent(&w.window().window_handle());
            }
            if let Some(files) = dialog.pick_files()
                && !files.is_empty()
            {
                (me.open_files)(files);
            }
        });
    }

    /// Folder picker for "Always in this folder"; choosing one also selects it.
    fn choose_folder_later(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        Timer::single_shot(Duration::ZERO, move || {
            let Some(me) = me.upgrade() else {
                return;
            };
            let current = (me.settings)().output.folder;
            let mut dialog = rfd::FileDialog::new().set_title(strings::CHOOSE_FOLDER_TITLE);
            if let Some(dir) = &current {
                dialog = dialog.set_directory(dir);
            }
            if let Some(w) = me.window.upgrade() {
                dialog = dialog.set_parent(&w.window().window_handle());
            }
            match dialog.pick_folder() {
                Some(folder) => {
                    me.with_ui(|ui| ui.set_output_mode(1));
                    me.save_with_folder(Some(folder));
                }
                // Cancelled: back to what is saved.
                None => me.with_ui(|ui| set_form(&ui, &Form::from_settings(&(me.settings)()))),
            }
        });
    }

    fn open_logs(&self) {
        let Some(root) = paths::app_data_dir() else {
            return;
        };
        let dir = paths::logs_dir(&root);
        let result = std::fs::create_dir_all(&dir)
            .map_err(anyhow::Error::from)
            .and_then(|()| shell::open_folder(&dir));
        if let Err(err) = result {
            applog::error("could not open the log folder", &err);
            self.with_ui(|ui| ui.set_save_error(shared(strings::OPEN_LOGS_FAILED)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_summary_shows_the_saved_defaults() {
        let f = Form::from_settings(&Settings::default());
        assert_eq!(
            defaults_summary(&f),
            "JPG 85 · WebP 80 · AVIF 60 · PNG level 2 · PDF Medium · Video Best (automatic) · MP3 192 kbps · 150 DPI"
        );
        // Out-of-range indices never panic.
        let odd = Form {
            pdf_preset: 99,
            mp3_bitrate: -1,
            ..f
        };
        assert!(defaults_summary(&odd).contains("PDF High"));
    }
}
