// SPDX-License-Identifier: GPL-3.0-or-later
//! Glue between the Slint window and the plain-Rust view models: fills the
//! window, reacts to its callbacks, runs the job and feeds its events back.
//!
//! Lifetime (SPEC §3.3): the event loop runs until [`App::quit`]. Closing the
//! window during a job hides it and shows the tray icon; the process then
//! exits when the job ends.

use crate::launch::Launch;
use crate::media_options::{self, AudioValues, MediaForm, MediaValues};
use crate::office_options::{self, OfficeForm, OfficeKind, OfficeValues};
use crate::options::{FormValues, ImageForm, ImageKind};
use crate::pdf_options::{self, PdfForm, PdfKind, PdfValues};
use crate::platform::{self, shell, toast, tray};
use crate::progress::{self, Progress};
use crate::results::{self, SavedResults};
use crate::view::{self, Mode, Selection};
use crate::{
    CategoryCard, ConvertUi, FileRow, FormatInfo, HomeUi, MainWindow, ProgressRow, ProgressUi,
    SkippedFile, Toggle, applog, focus, progress_text, strings,
};
use anyhow::{Context, Result};
use morph_core::image::options::{DEFAULT_ICO_SIZES, OutputFormat};
use morph_core::media;
use morph_core::office::options::DocFormat;
use morph_core::office::{Engines, LIBREOFFICE_DOWNLOAD_URL, detect};
use morph_core::scheduler::{self, JobConfig, JobHandle};
use morph_core::settings::{Settings, Theme};
use morph_core::state::{OutputChoice, State};
use morph_core::{Action, Event, Job, OutputSpec};
use morph_formats::{APP_DISPLAY_NAME, MenuGroup, Request, actions, paths};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PAGE_OPTIONS: i32 = 0;
const PAGE_PROGRESS: i32 = 1;
const PAGE_MESSAGE: i32 = 2;
/// Main window (Start menu launch): drop zone + Settings (`ui/shell_view.slint`).
const PAGE_HOME: i32 = 3;
/// Settings opened from the conversion window (gear button), with Back.
const PAGE_SETTINGS: i32 = 4;
/// How often job events are drained into the window.
const TICK: Duration = Duration::from_millis(100);
/// Cap per tick so a flood of events can't freeze the UI.
const MAX_EVENTS_PER_TICK: usize = 20_000;
/// Tray tooltip updates at most 2× per second.
const TRAY_INTERVAL: Duration = Duration::from_millis(500);
const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 3600);
/// Slint `category`: which option panels the window shows.
const CATEGORY_IMAGES: i32 = 0;
const CATEGORY_PDF: i32 = 1;
const CATEGORY_OFFICE: i32 = 2;
const CATEGORY_MEDIA: i32 = 3;

/// Which form the options page edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panels {
    Images,
    Pdf,
    Office,
    /// Video and audio (two tabs when both are selected).
    Media,
}

impl Panels {
    /// The panels of a mixed selection's tab.
    fn of(group: MenuGroup) -> Panels {
        match group {
            MenuGroup::Image => Panels::Images,
            MenuGroup::Pdf => Panels::Pdf,
            MenuGroup::Office => Panels::Office,
            MenuGroup::Video | MenuGroup::Audio => Panels::Media,
        }
    }

    fn category(self) -> i32 {
        match self {
            Panels::Images => CATEGORY_IMAGES,
            Panels::Pdf => CATEGORY_PDF,
            Panels::Office => CATEGORY_OFFICE,
            Panels::Media => CATEGORY_MEDIA,
        }
    }
}

/// The view globals of the window (`ui/convert_view.slint`,
/// `ui/progress_view.slint`).
pub trait Views {
    fn convert(&self) -> ConvertUi<'_>;
    fn progress(&self) -> ProgressUi<'_>;
}

impl Views for MainWindow {
    fn convert(&self) -> ConvertUi<'_> {
        self.global::<ConvertUi>()
    }
    fn progress(&self) -> ProgressUi<'_> {
        self.global::<ProgressUi>()
    }
}

/// `Theme` category id of a type tab (`ui/theme.slint`: 0 images, 1 PDF,
/// 2 video, 3 audio, 4 documents; 5 = mixed).
pub fn theme_category(group: MenuGroup) -> i32 {
    match group {
        MenuGroup::Image => 0,
        MenuGroup::Pdf => 1,
        MenuGroup::Video => 2,
        MenuGroup::Audio => 3,
        MenuGroup::Office => 4,
    }
}

/// `Theme` id of a mixed selection.
const THEME_MIXED: i32 = 5;

fn is_media(group: MenuGroup) -> bool {
    matches!(group, MenuGroup::Video | MenuGroup::Audio)
}

struct App {
    window: slint::Weak<MainWindow>,
    me: Weak<RefCell<App>>,
    app_data: Option<PathBuf>,
    config: JobConfig,
    /// `config.toml` (Settings page).
    settings: Settings,
    state: State,
    /// Files some panel takes (unsupported / turned off ones are in `skipped`).
    selection: Selection,
    /// Files skipped up front: unsupported, type turned off in Settings.
    skipped: Vec<(PathBuf, String)>,
    /// Mixed selection: the type tabs (empty = single type) and the shown one.
    tabs: Vec<MenuGroup>,
    tab: usize,
    /// Mixed selection: types the user unchecked (skipped by choice).
    excluded: Vec<MenuGroup>,
    /// The options page still holds the last job's files and options
    /// ("← Adjust options" on the finished view).
    can_adjust: bool,
    /// Indexes in `selection` of the files "Show files" lists (the ones that run).
    listed: Vec<usize>,
    /// The window was opened from the main window: Cancel / Close go back there.
    from_home: bool,
    panels: Panels,
    form: ImageForm,
    pdf_form: PdfForm,
    office_form: OfficeForm,
    media_form: MediaForm,
    /// `ffmpeg.exe` + `ffprobe.exe` were found (checked when media opens).
    ffmpeg_found: bool,
    /// Target formats in the document format box (depend on the files).
    office_targets: Vec<DocFormat>,
    /// Installed Office engines (registry lookup, cached; "Check again" refreshes).
    engines: Arc<Engines>,
    /// Chosen "Save to" folder (shown even while "next to originals" is selected).
    folder: Option<PathBuf>,
    files_model: Rc<VecModel<FileRow>>,
    skipped_model: Rc<VecModel<SkippedFile>>,
    /// Mixed selection: the category cards (edited in place: keeps focus).
    categories_model: Rc<VecModel<CategoryCard>>,
    ico_model: Rc<VecModel<Toggle>>,
    rows_model: Rc<VecModel<ProgressRow>>,
    job: Option<JobHandle>,
    progress: Option<Progress>,
    /// Results loaded from disk (`--show-results`).
    saved: Option<SavedResults>,
    /// Output folder of the running job (`None` = next to originals).
    job_folder: Option<PathBuf>,
    open_folder: Option<PathBuf>,
    timer: Timer,
    tray: Option<tray::Tray>,
    hidden: bool,
    last_tray_update: Option<Instant>,
}

/// Build the window for `launch` and run the event loop until the app quits.
pub fn run(launch: Launch) -> Result<()> {
    let app_data = paths::app_data_dir();
    let settings = app_data
        .as_deref()
        .map(|d| Settings::load(&paths::config_file(d)))
        .unwrap_or_default();
    // Settings → Performance and Office engine.
    let config = settings.job_config();
    scheduler::sweep_stale_temp(&config.temp_root, STALE_TEMP_AGE);
    let mut state = app_data
        .as_deref()
        .map(|d| State::load(&paths::state_file(d)))
        .unwrap_or_default();
    // Settings → Defaults: what an action starts with when nothing is remembered.
    state.set_defaults(&settings);

    let window = MainWindow::new().context("could not create the main window")?;
    window.set_window_title(shared(APP_DISPLAY_NAME));
    crate::convert_text::apply(&window);
    crate::progress_text::apply(&window);
    set_formats(&window, &settings);
    window.set_theme(theme_index(settings.appearance.theme));

    let files_model = Rc::new(VecModel::<FileRow>::default());
    let skipped_model = Rc::new(VecModel::<SkippedFile>::default());
    let ico_model = Rc::new(VecModel::<Toggle>::default());
    let categories_model = Rc::new(VecModel::<CategoryCard>::default());
    let rows_model = Rc::new(VecModel::<ProgressRow>::default());
    window
        .convert()
        .set_files(ModelRc::from(files_model.clone()));
    window
        .convert()
        .set_skipped(ModelRc::from(skipped_model.clone()));
    window
        .convert()
        .set_ico_sizes(ModelRc::from(ico_model.clone()));
    window
        .convert()
        .set_categories(ModelRc::from(categories_model.clone()));
    window
        .progress()
        .set_rows(ModelRc::from(rows_model.clone()));

    let app = Rc::new_cyclic(|me| {
        RefCell::new(App {
            window: window.as_weak(),
            me: me.clone(),
            app_data,
            config,
            settings,
            state,
            selection: Selection::default(),
            skipped: Vec::new(),
            tabs: Vec::new(),
            tab: 0,
            excluded: Vec::new(),
            can_adjust: false,
            listed: Vec::new(),
            from_home: false,
            panels: Panels::Images,
            form: ImageForm::default(),
            pdf_form: PdfForm::default(),
            office_form: OfficeForm::default(),
            media_form: MediaForm::default(),
            ffmpeg_found: true,
            office_targets: Vec::new(),
            engines: Arc::new(Engines::default()),
            folder: None,
            files_model,
            skipped_model,
            categories_model,
            ico_model,
            rows_model,
            job: None,
            progress: None,
            saved: None,
            job_folder: None,
            open_folder: None,
            timer: Timer::default(),
            tray: None,
            hidden: false,
            last_tray_update: None,
        })
    });

    for dir in requests_dirs() {
        Request::sweep(&dir, std::time::Duration::from_secs(24 * 3600));
    }
    {
        let mut a = app.borrow_mut();
        match launch {
            Launch::Request(path) => match Request::take(&path, &requests_dirs()) {
                Ok(request) => a.open_request(request),
                Err(err) => a.show_message(
                    strings::ERROR_REQUEST_UNREADABLE,
                    "",
                    &format!("{}: {err}", path.display()),
                ),
            },
            Launch::Files(files) => {
                a.open_request(Request::new(files, actions::MIXED_CONVERT, &[]));
            }
            Launch::ShowResults(id) => a.show_saved(&id),
            Launch::Empty => a.show_home(),
            Launch::OpenFolder(_) => {
                a.show_message(strings::EMPTY_HEADER, strings::EMPTY_PLACEHOLDER, "");
            }
        }
    }

    wire(&window, &app);
    let home = crate::home::wire(&window, home_hooks(&app));
    let expected = if window.get_page() == PAGE_HOME {
        platform::placement::MAIN_SIZE
    } else {
        platform::placement::CONVERSION_SIZE
    };
    platform::placement::place_on_cursor_monitor(&window, expected);
    window.show().context("could not show the window")?;
    let ran = slint::run_event_loop_until_quit().context("the window event loop failed");
    // A slider moved just before X / Esc: its delayed save still happens.
    home.flush_pending_save();
    ran?;
    // Keep `app` (and its timer/tray) alive until the loop ends.
    drop(app);
    Ok(())
}

/// Settings → Appearance as the window's `theme` (0 system, 1 light, 2 dark).
fn theme_index(theme: Theme) -> i32 {
    match theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    }
}

/// What the main window (`home.rs`) may ask of the app.
fn home_hooks(app: &Rc<RefCell<App>>) -> crate::home::HomeHooks {
    let weak = Rc::downgrade(app);
    let (w1, w2, w3) = (weak.clone(), weak.clone(), weak);
    crate::home::HomeHooks {
        open_files: Box::new(move |files| with_app(&w1, |a| a.open_files(files))),
        settings: Box::new(move || {
            let mut settings = Settings::default();
            with_app(&w2, |a| settings = a.settings.clone());
            settings
        }),
        save_settings: Box::new(move |settings| {
            let mut result = Err(std::io::Error::other("the app is busy"));
            with_app(&w3, |a| result = a.settings_changed(settings));
            result
        }),
    }
}

/// Run `f` on the app unless it is already busy (re-entrant call from a nested loop).
fn with_app(app: &Weak<RefCell<App>>, f: impl FnOnce(&mut App)) {
    if let Some(app) = app.upgrade()
        && let Ok(mut a) = app.try_borrow_mut()
    {
        f(&mut a);
    }
}

fn wire(window: &MainWindow, app: &Rc<RefCell<App>>) {
    let weak = Rc::downgrade(app);
    // `on!(view, on_x, |a, args| body)`: the callback runs `body` on the app.
    macro_rules! on {
        ($view:expr, $setter:ident, |$a:ident $(, $arg:ident)*| $body:expr) => {{
            let weak = weak.clone();
            $view.$setter(move |$($arg),*| with_app(&weak, |$a| $body));
        }};
    }
    let c = window.convert();
    on!(c, on_remove_file, |a, index| a.remove_file(index));
    on!(c, on_move_file, |a, from, to| a.move_file(from, to));
    on!(c, on_action_changed, |a| a.refresh());
    on!(c, on_type_tab_changed, |a| {
        let mut index = 0;
        a.with_window(|w| index = w.convert().get_type_tab());
        a.select_tab(usize::try_from(index).unwrap_or(0));
    });
    on!(c, on_category_included, |a, index, included| {
        a.set_category_included(index, included);
    });
    // Pure checks: no app state needed.
    c.on_validate_pages(|text| {
        SharedString::from(pdf_options::pages_error(&text).unwrap_or_default())
    });
    c.on_validate_split_ranges(|text| {
        SharedString::from(pdf_options::split_ranges_error(&text).unwrap_or_default())
    });
    c.on_validate_time(|text| {
        SharedString::from(media_options::start_error(&text).unwrap_or_default())
    });
    on!(c, on_select_folder, |a| {
        if a.folder.is_some() {
            a.with_window(|w| w.convert().set_use_folder(true));
        } else {
            a.pick_folder_later();
        }
    });
    on!(c, on_choose_folder, |a| a.pick_folder_later());
    on!(c, on_download_engine, |a| a.download_engine());
    on!(c, on_check_engines, |a| a.check_engines());
    on!(c, on_convert, |a| a.convert());
    on!(c, on_cancel, |a| a.close_or_go_home());
    on!(c, on_open_settings, |a| a.open_settings());

    let p = window.progress();
    on!(p, on_close_window, |a| a.close_or_go_home());
    on!(p, on_cancel_all, |a| a.cancel_job());
    on!(p, on_run_in_background, |a| {
        a.go_background(true);
    });
    on!(p, on_open_folder, |a| a.open_results_folder());
    on!(p, on_row_open, |a, index| a.row_open(index));
    on!(p, on_row_show, |a, index| a.row_show(index));
    on!(p, on_adjust_options, |a| a.adjust_options());

    // Settings inside the conversion window: Back (the rest of HomeUi is home.rs').
    on!(window.global::<HomeUi>(), on_back, |a| a.settings_back());
    on!(window, on_escape, |a| a.escape());

    let weak_close = weak.clone();
    window.window().on_close_requested(move || {
        let mut response = CloseRequestResponse::KeepWindowShown;
        with_app(&weak_close, |a| response = a.close_requested());
        response
    });
}

fn shared(text: &str) -> SharedString {
    SharedString::from(text)
}

fn list(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.iter().map(|s| shared(s)).collect::<Vec<_>>(),
    ))
}

/// The image format box. "Format default" quality = Settings → Defaults.
fn set_formats(window: &MainWindow, settings: &Settings) {
    use strings as s;
    // The image formats, then "PDF" (`options::PDF_FORMAT`, Image → PDF).
    let mut formats: Vec<FormatInfo> = OutputFormat::ALL
        .iter()
        .map(|f| FormatInfo {
            label: shared(s::format_label(*f)),
            lossy: f.is_lossy(),
            lossless_mode: f.has_lossless_mode(),
            alpha: f.supports_alpha(),
            default_quality: i32::from(settings.image_quality(*f)),
            is_ico: *f == OutputFormat::Ico,
            is_pdf: false,
        })
        .collect();
    formats.push(FormatInfo {
        label: shared(s::FORMAT_PDF),
        alpha: true,
        is_pdf: true,
        ..FormatInfo::default()
    });
    window
        .convert()
        .set_formats(ModelRc::new(VecModel::from(formats)));
}

/// `list` for owned strings.
fn list_owned(items: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.iter().map(|s| shared(s)).collect::<Vec<_>>(),
    ))
}

fn push_media_values(w: &ConvertUi, v: &MediaValues) {
    w.set_media_tab(v.tab);
    w.set_video_kind(v.video_kind);
    w.set_video_container(v.container);
    w.set_video_fast(v.fast);
    w.set_video_codec(v.video_codec);
    w.set_video_audio_codec(v.audio_codec);
    w.set_video_quality(v.quality);
    w.set_video_convert_encoder(v.convert_encoder);
    w.set_video_preset(v.preset);
    w.set_video_target_mb(shared(&v.target_mb));
    w.set_video_resolution(v.resolution);
    w.set_video_compress_encoder(v.compress_encoder);
    let e = &v.extract;
    w.set_extract_format(e.format);
    w.set_extract_bitrate(e.bitrate);
    w.set_extract_mp3_mode(e.mp3_mode);
    w.set_extract_sample_rate(e.sample_rate);
    w.set_extract_channels(e.channels);
    w.set_extract_keep_tags(e.keep_tags);
    let a = &v.audio;
    w.set_audio_format(a.format);
    w.set_audio_bitrate(a.bitrate);
    w.set_audio_mp3_mode(a.mp3_mode);
    w.set_audio_sample_rate(a.sample_rate);
    w.set_audio_channels(a.channels);
    w.set_audio_keep_tags(a.keep_tags);
    w.set_gif_start(shared(&v.gif_start));
    w.set_gif_length(v.gif_length);
    w.set_gif_width(v.gif_width);
    w.set_gif_fps(v.gif_fps);
}

fn read_media_values(w: &ConvertUi) -> MediaValues {
    MediaValues {
        tab: w.get_media_tab(),
        video_kind: w.get_video_kind(),
        container: w.get_video_container(),
        fast: w.get_video_fast(),
        video_codec: w.get_video_codec(),
        audio_codec: w.get_video_audio_codec(),
        quality: w.get_video_quality(),
        convert_encoder: w.get_video_convert_encoder(),
        preset: w.get_video_preset(),
        target_mb: w.get_video_target_mb().to_string(),
        resolution: w.get_video_resolution(),
        compress_encoder: w.get_video_compress_encoder(),
        extract: AudioValues {
            format: w.get_extract_format(),
            bitrate: w.get_extract_bitrate(),
            mp3_mode: w.get_extract_mp3_mode(),
            sample_rate: w.get_extract_sample_rate(),
            channels: w.get_extract_channels(),
            keep_tags: w.get_extract_keep_tags(),
        },
        audio: AudioValues {
            format: w.get_audio_format(),
            bitrate: w.get_audio_bitrate(),
            mp3_mode: w.get_audio_mp3_mode(),
            sample_rate: w.get_audio_sample_rate(),
            channels: w.get_audio_channels(),
            keep_tags: w.get_audio_keep_tags(),
        },
        gif_start: w.get_gif_start().to_string(),
        gif_length: w.get_gif_length(),
        gif_width: w.get_gif_width(),
        gif_fps: w.get_gif_fps(),
    }
}

fn push_values(w: &ConvertUi, v: &FormValues, ico: &VecModel<Toggle>) {
    w.set_kind(v.kind);
    w.set_format(v.format);
    w.set_quality(v.quality as f32);
    w.set_quality_custom(v.quality_custom);
    w.set_lossless(v.lossless);
    w.set_background_choice(v.background);
    ico.set_vec(
        DEFAULT_ICO_SIZES
            .iter()
            .zip(&v.ico_sizes)
            .map(|(size, checked)| Toggle {
                label: SharedString::from(size.to_string()),
                checked: *checked,
            })
            .collect::<Vec<_>>(),
    );
    w.set_svg_width(v.svg_width);
    w.set_convert_keep_metadata(v.convert_keep_metadata);
    w.set_convert_remove_gps(v.convert_remove_gps);
    w.set_image_pdf_page_size(v.pdf_page_size);
    w.set_image_pdf_margin(v.pdf_margin_mm);

    w.set_compress_mode(v.compress_mode);
    w.set_compress_quality(v.compress_quality as f32);
    w.set_compress_quality_custom(v.compress_quality_custom);
    w.set_target_value(shared(&v.target_value));
    w.set_target_unit(v.target_unit);
    w.set_convert_to_webp(v.convert_to_webp);
    w.set_png_lossy(v.png_lossy);
    w.set_compress_keep_metadata(v.compress_keep_metadata);
    w.set_compress_remove_gps(v.compress_remove_gps);

    w.set_resize_mode(v.resize_mode);
    w.set_percent(v.percent);
    w.set_resize_width(v.width);
    w.set_resize_height(v.height);
    w.set_keep_aspect(v.keep_aspect);
    w.set_no_enlarge(v.no_enlarge);
    w.set_resize_keep_metadata(v.resize_keep_metadata);
    w.set_resize_remove_gps(v.resize_remove_gps);

    w.set_strip(v.strip);
    w.set_rotate(v.rotate);
    w.set_flip_horizontal(v.flip_horizontal);
    w.set_flip_vertical(v.flip_vertical);
}

fn read_values(w: &ConvertUi, ico: &VecModel<Toggle>) -> FormValues {
    FormValues {
        kind: w.get_kind(),
        format: w.get_format(),
        quality: w.get_quality().round() as i32,
        quality_custom: w.get_quality_custom(),
        lossless: w.get_lossless(),
        background: w.get_background_choice(),
        ico_sizes: ico.iter().map(|t| t.checked).collect(),
        svg_width: w.get_svg_width(),
        convert_keep_metadata: w.get_convert_keep_metadata(),
        convert_remove_gps: w.get_convert_remove_gps(),
        pdf_page_size: w.get_image_pdf_page_size(),
        pdf_margin_mm: w.get_image_pdf_margin(),

        compress_mode: w.get_compress_mode(),
        compress_quality: w.get_compress_quality().round() as i32,
        compress_quality_custom: w.get_compress_quality_custom(),
        target_value: w.get_target_value().to_string(),
        target_unit: w.get_target_unit(),
        convert_to_webp: w.get_convert_to_webp(),
        png_lossy: w.get_png_lossy(),
        compress_keep_metadata: w.get_compress_keep_metadata(),
        compress_remove_gps: w.get_compress_remove_gps(),

        resize_mode: w.get_resize_mode(),
        percent: w.get_percent(),
        width: w.get_resize_width(),
        height: w.get_resize_height(),
        keep_aspect: w.get_keep_aspect(),
        no_enlarge: w.get_no_enlarge(),
        resize_keep_metadata: w.get_resize_keep_metadata(),
        resize_remove_gps: w.get_resize_remove_gps(),

        strip: w.get_strip(),
        rotate: w.get_rotate(),
        flip_horizontal: w.get_flip_horizontal(),
        flip_vertical: w.get_flip_vertical(),
    }
}

fn push_pdf_values(w: &ConvertUi, v: &PdfValues) {
    w.set_pdf_kind(v.kind);
    w.set_pdf_preset(v.preset);
    w.set_pdf_compress_dpi(v.compress_dpi);
    w.set_pdf_compress_quality(v.compress_quality as f32);
    w.set_pdf_image_format(v.image_format);
    w.set_pdf_dpi_choice(v.dpi_choice);
    w.set_pdf_dpi_custom(v.dpi_custom);
    w.set_pdf_pages(shared(&v.pages));
    w.set_pdf_page_separators(v.page_separators);
    w.set_pdf_markup_format(v.markup_format);
    w.set_pdf_markup_images(v.markup_images);
    w.set_pdf_split_mode(v.split_mode);
    w.set_pdf_split_every(v.split_every);
    w.set_pdf_split_ranges(shared(&v.split_ranges));
    w.set_pdf_page_size(v.page_size);
    w.set_pdf_margin(v.margin_mm);
    w.set_pdf_bookmarks(v.bookmarks);
}

fn read_pdf_values(w: &ConvertUi) -> PdfValues {
    PdfValues {
        kind: w.get_pdf_kind(),
        preset: w.get_pdf_preset(),
        compress_dpi: w.get_pdf_compress_dpi(),
        compress_quality: w.get_pdf_compress_quality().round() as i32,
        image_format: w.get_pdf_image_format(),
        dpi_choice: w.get_pdf_dpi_choice(),
        dpi_custom: w.get_pdf_dpi_custom(),
        pages: w.get_pdf_pages().to_string(),
        markup_format: w.get_pdf_markup_format(),
        markup_images: w.get_pdf_markup_images(),
        page_separators: w.get_pdf_page_separators(),
        split_mode: w.get_pdf_split_mode(),
        split_every: w.get_pdf_split_every(),
        split_ranges: w.get_pdf_split_ranges().to_string(),
        page_size: w.get_pdf_page_size(),
        margin_mm: w.get_pdf_margin(),
        bookmarks: w.get_pdf_bookmarks(),
    }
}

fn push_office_values(w: &ConvertUi, v: &OfficeValues, targets: &[DocFormat], files: &[PathBuf]) {
    // Short names for chips ("DOCX", "PDF").
    let short: Vec<&str> = targets
        .iter()
        .map(|f| strings::doc_format_short(*f))
        .collect();
    w.set_office_target_short(list(&short));
    w.set_office_csv_target(
        targets
            .iter()
            .position(|f| *f == DocFormat::Csv)
            .and_then(|i| i32::try_from(i).ok())
            .unwrap_or(-1),
    );
    w.set_office_md_target(
        targets
            .iter()
            .position(|f| *f == DocFormat::Md)
            .and_then(|i| i32::try_from(i).ok())
            .unwrap_or(-1),
    );
    let extras = office_options::markdown_extras(files);
    w.set_office_md_pictures(extras.pictures);
    w.set_office_md_sheets(extras.sheets);
    w.set_office_md_slides(extras.slides);
    w.set_office_images(v.images);
    w.set_office_max_rows(v.max_rows);
    w.set_office_slide_notes(v.slide_notes);
    w.set_office_csv_input(office_options::has_csv_input(files));
    w.set_office_has_images(office_options::has_image_input(files));
    w.set_office_kind(v.kind);
    w.set_office_format(v.format);
    w.set_office_delimiter(v.delimiter);
    w.set_office_csv_bom(v.csv_bom);
    w.set_office_image_format(v.image_format);
    w.set_office_dpi_choice(v.dpi_choice);
    w.set_office_dpi_custom(v.dpi_custom);
}

fn read_office_values(w: &ConvertUi) -> OfficeValues {
    OfficeValues {
        kind: w.get_office_kind(),
        format: w.get_office_format(),
        delimiter: w.get_office_delimiter(),
        csv_bom: w.get_office_csv_bom(),
        images: w.get_office_images(),
        max_rows: w.get_office_max_rows(),
        slide_notes: w.get_office_slide_notes(),
        image_format: w.get_office_image_format(),
        dpi_choice: w.get_office_dpi_choice(),
        dpi_custom: w.get_office_dpi_custom(),
    }
}

fn skipped_row((path, reason): &(PathBuf, String)) -> SkippedFile {
    SkippedFile {
        name: SharedString::from(
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .as_ref(),
        ),
        reason: SharedString::from(reason.as_str()),
    }
}

fn file_row(f: view::FileView) -> FileRow {
    FileRow {
        name: SharedString::from(f.name),
        folder: SharedString::from(f.folder),
        size: SharedString::from(f.size),
    }
}

/// Progress summary subtitle: where the files go.
fn saved_where(folder: Option<&Path>) -> SharedString {
    SharedString::from(match folder {
        Some(f) => strings::saved_to(&f.to_string_lossy()),
        None => strings::SAVED_NEXT_TO.to_owned(),
    })
}

/// Finished, but not everything converted: the summary shows a warning badge.
fn had_problems(s: &morph_core::Summary) -> bool {
    s.failed > 0 || s.cancelled > 0 || s.done == 0
}

/// [`had_problems`] for results loaded from disk.
fn saved_had_problems(saved: &SavedResults) -> bool {
    use results::RowStatus;
    saved
        .rows
        .iter()
        .any(|r| matches!(r.status, RowStatus::Failed | RowStatus::Cancelled))
        || !saved.rows.iter().any(|r| r.status == RowStatus::Done)
}

fn path_text(path: Option<&Path>) -> SharedString {
    path.map(|p| SharedString::from(p.to_string_lossy().as_ref()))
        .unwrap_or_default()
}

impl App {
    fn with_window(&self, f: impl FnOnce(&MainWindow)) {
        if let Some(w) = self.window.upgrade() {
            f(&w);
        }
    }

    // ── Opening ──────────────────────────────────────────────────────────────

    fn open_request(&mut self, request: Request) {
        let mut show_files = false;
        let opening = view::open(&request, &self.settings.menu);
        let files = opening.files;
        self.skipped = opening.skipped;
        self.tabs = opening.tabs;
        self.tab = 0;
        self.excluded.clear();
        self.can_adjust = false;
        self.listed.clear();
        match opening.mode {
            Mode::Images(kind) => {
                self.panels = Panels::Images;
                self.form = ImageForm::prefill(&self.state, kind, &request.preset);
            }
            Mode::Pdf(kind) => {
                self.panels = Panels::Pdf;
                self.pdf_form = PdfForm::prefill(&self.state, kind, &request.preset);
                // Merge starts in natural order (SPEC §3.2); show it, it matters.
                show_files = kind == PdfKind::Merge && files.len() > 1;
                if kind == PdfKind::ToWord {
                    self.engines = detect::engines();
                }
            }
            Mode::Office(kind) => {
                self.panels = Panels::Office;
                self.engines = detect::engines();
                self.office_form = OfficeForm::prefill(
                    &self.state,
                    kind,
                    &request.preset,
                    &files,
                    &self.engines,
                    self.config.office_engine,
                );
            }
            Mode::Video(_) | Mode::Audio => {
                self.panels = Panels::Media;
                self.ffmpeg_found = media::ffmpeg::find().is_some();
                self.media_form =
                    MediaForm::prefill(&self.state, &request.action, &request.preset, &files);
            }
            Mode::Mixed => self.prefill_tabs(&files),
            Mode::Unavailable(message) => {
                self.show_message(strings::NOT_AVAILABLE_TITLE, &message, "");
                return;
            }
        }
        self.selection = Selection::from_files(files);
        if self.panels == Panels::Pdf && self.tabs.is_empty() {
            self.selection.sort_natural();
        }
        // "Save to": the remembered choice (Remember on), else Settings → Output.
        let remembered = self.state.remember && self.state.output != OutputChoice::default();
        let output = if remembered {
            self.state.output.clone()
        } else {
            OutputChoice {
                use_folder: self.settings.output.use_folder,
                folder: self.settings.output.folder.clone(),
            }
        };
        self.folder = output.folder.clone().filter(|f| f.is_dir());
        let use_folder = output.use_folder && self.folder.is_some();
        let mut values = FormValues::from_form(&self.form);
        values.show_default_quality(|f| self.settings.image_quality(f));
        let pdf_values = PdfValues::from_form(&self.pdf_form);
        self.office_targets = office_options::targets(&self.selection.files);
        let office_values = OfficeValues::from_form(&self.office_form, &self.office_targets);
        let media_values = MediaValues::from_form(&self.media_form);
        self.with_window(|w| {
            set_formats(w, &self.settings);
            push_values(&w.convert(), &values, &self.ico_model);
            push_pdf_values(&w.convert(), &pdf_values);
            push_office_values(
                &w.convert(),
                &office_values,
                &self.office_targets,
                &self.selection.files,
            );
            push_media_values(&w.convert(), &media_values);
            w.convert().set_media_advanced(false);
            w.convert().set_ffmpeg_missing(!self.ffmpeg_found);
            w.convert().set_form_error(SharedString::default());
            w.convert().set_files_open(show_files);
            w.convert().set_use_folder(use_folder);
            w.convert()
                .set_folder_text(path_text(self.folder.as_deref()));
            w.convert().set_remember(self.state.remember);
            w.set_page(PAGE_OPTIONS);
        });
        self.select_tab(0);
    }

    /// Mixed selection: every tab's form starts from its default action
    /// (remembered options / Settings → Defaults on top).
    fn prefill_tabs(&mut self, files: &[PathBuf]) {
        let none = Default::default();
        self.form = ImageForm::prefill(&self.state, ImageKind::Convert, &none);
        self.pdf_form = PdfForm::prefill(&self.state, PdfKind::Compress, &none);
        let has = |g: MenuGroup| self.tabs.contains(&g);
        if has(MenuGroup::Office) || has(MenuGroup::Pdf) {
            self.engines = detect::engines();
        }
        self.office_form = OfficeForm::prefill(
            &self.state,
            OfficeKind::Convert,
            &none,
            files,
            &self.engines,
            self.config.office_engine,
        );
        if has(MenuGroup::Video) || has(MenuGroup::Audio) {
            self.ffmpeg_found = media::ffmpeg::find().is_some();
        }
        self.media_form = MediaForm::prefill(&self.state, actions::VIDEO_CONVERT, &none, files);
        self.panels = self.tabs.first().copied().map_or(self.panels, Panels::of);
    }

    /// Show tab `index` of a mixed selection (single type: just the panels).
    fn select_tab(&mut self, index: usize) {
        if let Some(group) = self.tabs.get(index).copied() {
            self.tab = index;
            self.panels = Panels::of(group);
            let tab = i32::try_from(index).unwrap_or(0);
            self.with_window(|w| {
                w.convert().set_type_tab(tab);
                if is_media(group) {
                    w.convert()
                        .set_media_tab(i32::from(group == MenuGroup::Audio));
                }
            });
        }
        let category = self.panels.category();
        self.with_window(|w| w.convert().set_category(category));
        self.refresh();
    }

    /// The tab shown now (`None`: single-type window).
    fn current_group(&self) -> Option<MenuGroup> {
        self.tabs.get(self.tab).copied()
    }

    /// The action a panel has selected right now, for the counts (default
    /// options; for documents the chosen target format, which decides what
    /// can run). `group` picks video or audio on the media panels.
    fn preview_action(&self, w: &MainWindow, panels: Panels, group: MenuGroup) -> Option<Action> {
        // The widgets' current values on a copy of the form (the mixed cards
        // show the chosen format). Invalid values (e.g. a number being typed)
        // don't change what runs.
        let c = w.convert();
        Some(match panels {
            Panels::Images => {
                let mut form = self.form.clone();
                let _ = read_values(&c, &self.ico_model).apply(&mut form);
                form.action()
            }
            Panels::Pdf => {
                let mut form = self.pdf_form.clone();
                let _ = read_pdf_values(&c).apply(&mut form);
                form.action()
            }
            Panels::Office => {
                let mut form = self.office_form.clone();
                let _ = read_office_values(&c).apply(&mut form, &self.office_targets);
                form.action()
            }
            Panels::Media => {
                let mut form = self.media_form.clone();
                let _ = read_media_values(&c).apply(&mut form, &self.selection.files);
                if group == MenuGroup::Audio {
                    form.audio_action()
                } else {
                    form.video_action()
                }
            }
        })
    }

    /// Read and check a panel's widgets into its form: the action to run, or
    /// the message to show.
    fn checked_action(
        &mut self,
        w: &MainWindow,
        panels: Panels,
        group: MenuGroup,
    ) -> Result<Action, String> {
        match panels {
            Panels::Images => read_values(&w.convert(), &self.ico_model)
                .apply(&mut self.form)
                .map(|()| self.form.action())
                .map_err(str::to_owned),
            Panels::Pdf => read_pdf_values(&w.convert())
                .apply(&mut self.pdf_form)
                .map(|()| self.pdf_form.action()),
            Panels::Office => read_office_values(&w.convert())
                .apply(&mut self.office_form, &self.office_targets)
                .map(|()| self.office_form.action()),
            Panels::Media => read_media_values(&w.convert())
                .apply(&mut self.media_form, &self.selection.files)
                .map(|()| match group {
                    MenuGroup::Audio => self.media_form.audio_action(),
                    _ => self.media_form.video_action(),
                }),
        }
    }

    /// The job for `actions`: one per tab of a mixed selection, else the
    /// window's action(s) on every file.
    fn plan(&self, actions: &[(MenuGroup, Action)], output: &OutputSpec) -> view::Planned {
        if self.tabs.is_empty() {
            let list: Vec<Action> = actions.iter().map(|(_, a)| a.clone()).collect();
            view::single_job(
                &self.selection.files,
                &self.skipped,
                &list,
                &self.engines,
                self.config.office_engine,
                output,
            )
        } else {
            view::mixed_job(
                &self.selection.files,
                &self.skipped,
                actions,
                &self.excluded,
                strings::FFMPEG_MISSING,
                &self.engines,
                self.config.office_engine,
                output,
            )
        }
    }

    /// What each tab (or the single panel) would run now, for the counts.
    fn preview_actions(&self, w: &MainWindow) -> Vec<(MenuGroup, Action)> {
        if self.tabs.is_empty() {
            if self.panels == Panels::Media {
                let mut form = self.media_form.clone();
                let _ = read_media_values(&w.convert()).apply(&mut form, &self.selection.files);
                return form
                    .actions(&self.selection.files)
                    .into_iter()
                    .map(|a| (MenuGroup::Video, a))
                    .collect();
            }
            // The group is unused for a single-type window.
            return self
                .preview_action(w, self.panels, MenuGroup::Image)
                .map(|a| (MenuGroup::Image, a))
                .into_iter()
                .collect();
        }
        self.tabs
            .iter()
            .filter(|g| self.ffmpeg_found || !is_media(**g))
            .filter_map(|g| self.preview_action(w, Panels::of(*g), *g).map(|a| (*g, a)))
            .collect()
    }

    fn has_office_panel(&self) -> bool {
        self.panels == Panels::Office || self.tabs.contains(&MenuGroup::Office)
    }

    /// The files changed: keep the document format if it still fits.
    fn refit_office_targets(&mut self) {
        if !self.has_office_panel() {
            return;
        }
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let _ = read_office_values(&w.convert()).apply(&mut self.office_form, &self.office_targets);
        self.office_form.fit_to(
            &self.selection.files,
            &self.engines,
            self.config.office_engine,
        );
        self.office_targets = office_options::targets(&self.selection.files);
        let values = OfficeValues::from_form(&self.office_form, &self.office_targets);
        push_office_values(
            &w.convert(),
            &values,
            &self.office_targets,
            &self.selection.files,
        );
    }

    fn download_engine(&self) {
        if let Err(err) = shell::open_url(LIBREOFFICE_DOWNLOAD_URL) {
            applog::error("could not open the LibreOffice download page", &err);
        }
    }

    /// "Check again" after installing an engine (Office, LibreOffice, or
    /// FFmpeg next to Morph): the same detection as Settings → Check again.
    fn check_engines(&mut self) {
        self.engines = detect::refresh();
        self.ffmpeg_found = media::ffmpeg::find().is_some();
        let missing = !self.ffmpeg_found;
        self.with_window(|w| w.convert().set_ffmpeg_missing(missing));
        self.refresh();
    }

    /// Mixed selection: card `index`'s type is included (runs) or skipped.
    fn set_category_included(&mut self, index: i32, included: bool) {
        let Some(group) = usize::try_from(index)
            .ok()
            .and_then(|i| self.tabs.get(i).copied())
        else {
            return;
        };
        self.excluded.retain(|g| *g != group);
        if !included {
            self.excluded.push(group);
        }
        self.listed.clear();
        self.refresh();
    }

    /// The category cards of a mixed selection: count, what runs, included.
    fn refresh_categories(&self, actions: &[(MenuGroup, Action)]) {
        let files = &self.selection.files;
        let cards: Vec<CategoryCard> = self
            .tabs
            .iter()
            .map(|g| CategoryCard {
                category: theme_category(*g),
                label: SharedString::from(view::tab_label(*g, files)),
                summary: actions
                    .iter()
                    .find(|(group, _)| group == g)
                    .map(|(_, a)| SharedString::from(view::action_summary(a)))
                    .unwrap_or_default(),
                included: !self.excluded.contains(g),
                include_label: SharedString::from(strings::include_category(view::group_name(*g))),
            })
            .collect();
        if cards.len() == self.categories_model.row_count() {
            for (i, card) in cards.into_iter().enumerate() {
                if self.categories_model.row_data(i).as_ref() != Some(&card) {
                    self.categories_model.set_row_data(i, card);
                }
            }
        } else {
            self.categories_model.set_vec(cards);
        }
    }

    /// The type a single-type window shows (summary header badge / subtitle).
    fn single_group(&self) -> MenuGroup {
        use morph_formats::Category;
        match self.panels {
            Panels::Images => MenuGroup::Image,
            Panels::Pdf => MenuGroup::Pdf,
            Panels::Office => MenuGroup::Office,
            Panels::Media if media_options::count(&self.selection.files, Category::Video) > 0 => {
                MenuGroup::Video
            }
            Panels::Media => MenuGroup::Audio,
        }
    }

    /// Mixed selection: tabs whose files all got removed disappear; the
    /// labels show the counts.
    fn refresh_tabs(&mut self, w: &MainWindow) {
        let present = view::tabs_present(&self.selection.files);
        if !present.is_empty() && present != self.tabs {
            let shown = self.current_group();
            self.tabs = present;
            self.tab = shown
                .and_then(|g| self.tabs.iter().position(|t| *t == g))
                .unwrap_or(0);
            if let Some(group) = self.current_group() {
                self.panels = Panels::of(group);
                w.convert()
                    .set_type_tab(i32::try_from(self.tab).unwrap_or(0));
                w.convert().set_category(self.panels.category());
                if is_media(group) {
                    w.convert()
                        .set_media_tab(i32::from(group == MenuGroup::Audio));
                }
            }
        }
        let labels: Vec<String> = self
            .tabs
            .iter()
            .map(|g| view::tab_label(*g, &self.selection.files))
            .collect();
        w.convert().set_type_tabs(list_owned(&labels));
    }

    /// Everything that depends on the files and the chosen actions: the file
    /// list and Skipped list, header, skip note, engine banner, Convert button.
    fn refresh(&mut self) {
        use morph_formats::Category;
        let Some(w) = self.window.upgrade() else {
            return;
        };
        if !self.tabs.is_empty() {
            self.refresh_tabs(&w);
        }
        let actions = self.preview_actions(&w);
        self.refresh_categories(&actions);
        let planned = self.plan(&actions, &OutputSpec::default());
        let runs = planned.runs();
        let listed: Vec<usize> = (0..self.selection.files.len())
            .filter(|i| runs.contains(self.selection.files[*i].as_path()))
            .collect();
        drop(runs);
        if listed != self.listed || self.files_model.row_count() != listed.len() {
            let views = self.selection.file_views();
            let rows: Vec<FileRow> = listed
                .iter()
                .filter_map(|i| views.get(*i).cloned())
                .map(file_row)
                .collect();
            self.files_model.set_vec(rows);
            self.listed = listed;
        }
        let skipped = &planned.job.skipped;
        // The header counts the files waiting for an Office engine too: the
        // banner explains why they don't run (Convert stays disabled if
        // nothing else does).
        let waiting: HashSet<&Path> = skipped
            .iter()
            .filter(|(_, reason)| reason == strings::NEEDS_ENGINE)
            .map(|(path, _)| path.as_path())
            .collect();
        let header_files: Vec<usize> = (0..self.selection.files.len())
            .filter(|i| {
                self.listed.contains(i) || waiting.contains(self.selection.files[*i].as_path())
            })
            .collect();
        self.skipped_model
            .set_vec(skipped.iter().map(skipped_row).collect::<Vec<_>>());
        let count = self.listed.len();

        let mixed = !self.tabs.is_empty();
        let group = self.current_group();
        let (missing, engines) = planned.engine(group);
        let in_tab = match group {
            Some(g) => self
                .listed
                .iter()
                .filter(|i| view::group_of(&self.selection.files[**i]) == Some(g))
                .count(),
            None => count,
        };
        let engine_note = if missing > 0 && in_tab > 0 {
            strings::engine_missing_some(missing)
        } else {
            String::new()
        };
        let (skip_note, label) = if mixed {
            (
                strings::will_skip_mixed(skipped.len()),
                if count == 0 {
                    strings::CONVERT_NOTHING.to_owned()
                } else {
                    strings::convert_all_button(count)
                },
            )
        } else {
            // Files waiting for an engine are explained by the banner instead.
            let note = strings::will_skip(skipped.len().saturating_sub(missing));
            let multi = actions.iter().any(|(_, a)| a.is_multi_input());
            let label = if count == 0 {
                strings::CONVERT_NOTHING.to_owned()
            } else if multi {
                strings::merge_button(count)
            } else {
                strings::convert_button(count)
            };
            (note, label)
        };
        let media_blocked = !mixed && self.panels == Panels::Media && !self.ffmpeg_found;
        let files = &self.selection.files;
        let c = w.convert();
        c.set_header_count(SharedString::from(strings::file_count(header_files.len())));
        c.set_header_size(SharedString::from(view::format_size(
            self.selection.size_of(&header_files),
        )));
        if mixed {
            c.set_header_category(THEME_MIXED);
            c.set_header_subtitle(SharedString::from(strings::categories_selected(
                self.tabs.len(),
            )));
        } else {
            let group = self.single_group();
            c.set_header_category(theme_category(group));
            c.set_header_subtitle(SharedString::from(strings::batch_ready(view::group_name(
                group,
            ))));
        }
        w.convert().set_skip_note(SharedString::from(skip_note));
        w.convert()
            .set_skipped_heading(SharedString::from(strings::skipped_heading(skipped.len())));
        w.convert().set_has_svg(self.selection.has_svg());
        let pdfs = files
            .iter()
            .filter(|f| view::group_of(f) == Some(MenuGroup::Pdf))
            .count();
        w.convert()
            .set_pdf_count(i32::try_from(pdfs).unwrap_or(i32::MAX));
        w.convert().set_convert_label(SharedString::from(label));
        w.convert().set_convert_enabled(count > 0 && !media_blocked);
        w.convert().set_engine_missing(missing > 0);
        w.convert()
            .set_engine_missing_note(SharedString::from(engine_note));
        w.convert()
            .set_engine_line(SharedString::from(strings::using_engines(&engines)));
        // Video + audio in a single window (not from the type tabs).
        let videos = media_options::count(files, Category::Video);
        let audios = media_options::count(files, Category::Audio);
        let labels = [strings::tab_video(videos), strings::tab_audio(audios)];
        w.convert()
            .set_media_tabs_visible(!mixed && media_options::is_mixed(files));
        w.convert()
            .set_media_tab_labels(list(&[labels[0].as_str(), labels[1].as_str()]));
        w.convert()
            .set_audio_lossy_input(media_options::has_lossy_audio(files));
        if !mixed && self.panels == Panels::Media {
            // The shown tab lost all its files: show the other one.
            if videos == 0 && audios > 0 {
                w.convert().set_media_tab(1);
            } else if audios == 0 && videos > 0 {
                w.convert().set_media_tab(0);
            }
        }
    }

    /// Gear button: Settings inside the conversion window. The options page
    /// keeps its state (it isn't rebuilt); Back returns to it.
    fn open_settings(&mut self) {
        if self.is_running() {
            return;
        }
        self.with_window(|w| w.set_page(PAGE_SETTINGS));
        // Refresh the page's values and engine status. Later: `home.rs` reads
        // the settings through a hook that borrows the app, busy right now.
        let window = self.window.clone();
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(w) = window.upgrade() {
                w.global::<HomeUi>().invoke_show_settings();
            }
        });
    }

    /// Settings → Back: the selection as it was, with what Settings changed
    /// applied (Office engine, default qualities, theme).
    fn settings_back(&mut self) {
        let mut page = PAGE_OPTIONS;
        self.with_window(|w| page = w.get_page());
        if page != PAGE_SETTINGS {
            return;
        }
        self.with_window(|w| {
            set_formats(w, &self.settings);
            w.set_page(PAGE_OPTIONS);
        });
        self.refresh();
    }

    /// Finished view → "← Adjust options": the options page with the same
    /// files and options, ready to run again.
    fn adjust_options(&mut self) {
        if self.is_running() || !self.can_adjust {
            return;
        }
        self.progress = None;
        self.saved = None;
        self.open_folder = None;
        self.job_folder = None;
        self.rows_model.set_vec(Vec::new());
        self.with_window(|w| {
            w.progress().set_details_open(false);
            w.progress().set_can_adjust(false);
            w.set_page(PAGE_OPTIONS);
        });
        self.listed.clear();
        self.refresh();
    }

    /// The main window (Start menu launch): drop zone + Settings.
    fn show_home(&mut self) {
        self.from_home = true;
        self.with_window(|w| w.set_page(PAGE_HOME));
    }

    /// Files dropped on (or browsed from) the main window: the same flow as a
    /// right-click on them (SPEC §3.4).
    fn open_files(&mut self, files: Vec<PathBuf>) {
        if files.is_empty() || self.is_running() {
            return;
        }
        self.open_request(Request::new(files, actions::MIXED_CONVERT, &[]));
    }

    /// Cancel on the options page, Close after the summary or on a message:
    /// back to the drop zone when the window came from there, else quit.
    fn close_or_go_home(&mut self) {
        if !self.from_home || self.is_running() || self.hidden {
            self.quit();
            return;
        }
        self.progress = None;
        self.saved = None;
        self.open_folder = None;
        self.rows_model.set_vec(Vec::new());
        self.with_window(|w| {
            w.progress().set_details_open(false);
            w.convert().set_files_open(false);
        });
        self.show_home();
    }

    /// Settings page saved: write `config.toml` (the shell DLL picks up the
    /// file types by its modified time) and use the new values from now on.
    /// A changed Settings → Output "Save to" (mode or folder) forgets the
    /// remembered "Save to", so the new default shows next time.
    fn settings_changed(&mut self, settings: Settings) -> std::io::Result<()> {
        if settings == self.settings {
            return Ok(());
        }
        let saved = match &self.app_data {
            Some(dir) => settings.save(&paths::config_file(dir)),
            None => Err(std::io::Error::other("no app data folder")),
        };
        if let Err(err) = &saved {
            applog::error("could not save config.toml", err);
        }
        let output_changed = !settings.output.same_save_to(&self.settings.output);
        self.config = JobConfig {
            temp_root: self.config.temp_root.clone(),
            ..settings.job_config()
        };
        self.state.set_defaults(&settings);
        if output_changed && self.state.output != OutputChoice::default() {
            self.state.output = OutputChoice::default();
            if let Some(dir) = &self.app_data
                && let Err(err) = self.state.save(&paths::state_file(dir))
            {
                applog::error("could not save state.toml", &err);
            }
        }
        self.with_window(|w| w.set_theme(theme_index(settings.appearance.theme)));
        self.settings = settings;
        saved
    }

    fn show_message(&mut self, title: &str, message: &str, detail: &str) {
        self.with_window(|w| {
            w.progress().set_message_title(shared(title));
            w.progress().set_message(shared(message));
            w.progress().set_message_detail(shared(detail));
            w.set_page(PAGE_MESSAGE);
        });
    }

    fn show_saved(&mut self, id: &str) {
        let loaded = self
            .app_data
            .as_deref()
            .map(|d| SavedResults::load(&paths::results_dir(d), id));
        let saved = match loaded {
            Some(Ok(saved)) => saved,
            Some(Err(err)) => {
                applog::error("could not load saved results", &err);
                self.show_message(
                    strings::RESULTS_MISSING,
                    strings::RESULTS_MISSING_DETAIL,
                    "",
                );
                return;
            }
            None => {
                self.show_message(
                    strings::RESULTS_MISSING,
                    strings::RESULTS_MISSING_DETAIL,
                    "",
                );
                return;
            }
        };
        self.rows_model.set_vec(
            saved
                .rows
                .iter()
                .map(|r| progress_text::row(progress::saved_row_view(r)))
                .collect::<Vec<_>>(),
        );
        self.open_folder = saved.folder.clone();
        self.can_adjust = false;
        self.with_window(|w| {
            w.progress().set_can_adjust(false);
            w.progress().set_subtitle(SharedString::default());
            w.progress().set_progress_title(shared(&saved.summary));
            w.progress().set_progress_count(SharedString::default());
            w.progress().set_overall(1.0);
            w.progress().set_finished(true);
            w.progress().set_had_problems(saved_had_problems(&saved));
            w.progress().set_can_open_folder(saved.folder.is_some());
            w.set_page(PAGE_PROGRESS);
        });
        self.saved = Some(saved);
    }

    // ── Options page ─────────────────────────────────────────────────────────

    fn remove_file(&mut self, index: i32) {
        let Some(i) = usize::try_from(index)
            .ok()
            .and_then(|i| self.listed.get(i).copied())
        else {
            return;
        };
        self.selection.remove(i);
        self.listed.clear();
        self.refit_office_targets();
        self.refresh();
    }

    /// Merge order: move one file (drag or the arrow buttons). The model is
    /// edited in place so the list keeps its scroll position.
    fn move_file(&mut self, from: i32, to: i32) {
        let index = |i: i32| usize::try_from(i).ok().filter(|i| *i < self.listed.len());
        let (Some(from), Some(to)) = (index(from), index(to)) else {
            return;
        };
        if !self.selection.move_file(self.listed[from], self.listed[to]) {
            return;
        }
        if self.listed.len() == self.selection.files.len() {
            // Every file is listed: the indexes match.
            let row = self.files_model.remove(from);
            self.files_model.insert(to, row);
        } else {
            self.listed.clear();
            self.refresh();
        }
    }

    /// Open the folder picker after this callback returns: the dialog runs a
    /// nested message loop, and the app must not be borrowed meanwhile.
    fn pick_folder_later(&self) {
        let me = self.me.clone();
        let start = self.folder.clone();
        let window = self.window.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let mut dialog = rfd::FileDialog::new().set_title(strings::PICK_FOLDER_TITLE);
            if let Some(dir) = &start {
                dialog = dialog.set_directory(dir);
            }
            if let Some(w) = window.upgrade() {
                dialog = dialog.set_parent(&w.window().window_handle());
            }
            let picked = dialog.pick_folder();
            with_app(&me, |a| a.folder_picked(picked));
        });
    }

    fn folder_picked(&mut self, picked: Option<PathBuf>) {
        if let Some(folder) = picked {
            self.folder = Some(folder);
        }
        let has_folder = self.folder.is_some();
        self.with_window(|w| {
            w.convert()
                .set_folder_text(path_text(self.folder.as_deref()));
            // Cancelled with nothing chosen: stay "next to the originals".
            w.convert().set_use_folder(has_folder);
        });
    }

    fn remember(&mut self, actions: &[Action], use_folder: bool, remember: bool) {
        self.state.remember = remember;
        if remember {
            for action in actions {
                self.state.remember_action(action);
            }
            self.state.output = OutputChoice {
                use_folder,
                folder: self
                    .folder
                    .clone()
                    .or_else(|| self.state.output.folder.clone()),
            };
        }
        if let Some(dir) = &self.app_data
            && let Err(err) = self.state.save(&paths::state_file(dir))
        {
            applog::error("could not save state.toml", &err);
        }
    }

    fn convert(&mut self) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        if w.get_page() != PAGE_OPTIONS || self.job.is_some() {
            return;
        }
        let actions = match self.checked_actions(&w) {
            Ok(actions) => actions,
            Err(message) => {
                w.convert().set_form_error(SharedString::from(message));
                return;
            }
        };
        w.convert().set_form_error(SharedString::default());
        let use_folder = w.convert().get_use_folder() && self.folder.is_some();
        let job_folder = if use_folder {
            self.folder.clone()
        } else {
            None
        };
        let list: Vec<Action> = actions.iter().map(|(_, a)| a.clone()).collect();
        self.remember(&list, use_folder, w.convert().get_remember());

        // Files already in the target format, without an engine or of no tab
        // are skipped up front with their reason; the rest run as usual.
        let output = OutputSpec {
            folder: job_folder.clone(),
        };
        let job = self.plan(&actions, &output).job;
        if job.tasks.is_empty() {
            return;
        }
        let multi = self.tabs.is_empty() && list.iter().any(Action::is_multi_input);
        self.start_job(&w, job, job_folder, multi);
    }

    /// Check every form that runs and return its action: one per tab of a
    /// mixed selection, else the window's action(s). On a bad value the tab
    /// with it is shown and its message returned.
    fn checked_actions(&mut self, w: &MainWindow) -> Result<Vec<(MenuGroup, Action)>, String> {
        if self.tabs.is_empty() {
            if self.panels == Panels::Media {
                if !self.ffmpeg_found {
                    return Ok(Vec::new());
                }
                read_media_values(&w.convert())
                    .apply(&mut self.media_form, &self.selection.files)?;
                return Ok(self
                    .media_form
                    .actions(&self.selection.files)
                    .into_iter()
                    .map(|a| (MenuGroup::Video, a))
                    .collect());
            }
            let action = self.checked_action(w, self.panels, MenuGroup::Image)?;
            return Ok(vec![(MenuGroup::Image, action)]);
        }
        let mut list = Vec::new();
        for (i, group) in self.tabs.clone().into_iter().enumerate() {
            if (is_media(group) && !self.ffmpeg_found) || self.excluded.contains(&group) {
                continue;
            }
            match self.checked_action(w, Panels::of(group), group) {
                Ok(action) => list.push((group, action)),
                Err(message) => {
                    if i != self.tab {
                        self.select_tab(i);
                    }
                    return Err(message);
                }
            }
        }
        Ok(list)
    }

    /// Switch to the progress page and run `job`.
    fn start_job(
        &mut self,
        w: &MainWindow,
        mut job: Job,
        job_folder: Option<PathBuf>,
        multi: bool,
    ) {
        // Image quality left at the format default, "use GPU", keep dates.
        self.settings.apply_to_job(&mut job);
        let progress = Progress::new(&job);
        self.rows_model.set_vec(
            (0..progress.rows().len())
                .map(|i| progress_text::row(progress.row_view(i)))
                .collect::<Vec<_>>(),
        );
        let title = match job.tasks.first() {
            Some(task) if multi => strings::merging(task.inputs.len()),
            _ => strings::converting(progress.total()),
        };
        w.progress().set_progress_title(SharedString::from(title));
        w.progress()
            .set_progress_count(SharedString::from(format!("0 / {}", progress.total())));
        w.progress().set_overall(0.0);
        w.progress().set_overall_indeterminate(false);
        w.progress().set_finished(false);
        w.progress().set_had_problems(false);
        w.progress().set_cancelling(false);
        w.progress()
            .set_subtitle(saved_where(job_folder.as_deref()));
        // The button sits in the finished footer: shown once the job ends.
        w.progress().set_can_adjust(true);
        w.set_page(PAGE_PROGRESS);
        self.can_adjust = true;

        self.job = Some(scheduler::run_job(job, self.config.clone()));
        self.progress = Some(progress);
        self.job_folder = job_folder;
        let me = self.me.clone();
        self.timer.start(TimerMode::Repeated, TICK, move || {
            with_app(&me, App::tick);
        });
    }

    // ── Running job ──────────────────────────────────────────────────────────

    fn tick(&mut self) {
        let (Some(job), Some(progress)) = (&self.job, &mut self.progress) else {
            return;
        };
        for _ in 0..MAX_EVENTS_PER_TICK {
            match job.events.try_recv() {
                Ok(event) => progress.apply(event),
                Err(err) => {
                    if err.is_disconnected() && !progress.is_finished() {
                        // The job thread is gone without a summary (couldn't start).
                        let summary = progress.summary_from_rows();
                        progress.apply(Event::JobFinished { summary });
                    }
                    break;
                }
            }
        }
        for i in progress.take_dirty() {
            self.rows_model
                .set_row_data(i, progress_text::row(progress.row_view(i)));
        }
        let (finished, total, fraction) =
            (progress.finished(), progress.total(), progress.fraction());
        let indeterminate = progress.indeterminate();
        self.with_window(|w| {
            w.progress()
                .set_progress_count(SharedString::from(format!("{finished} / {total}")));
            w.progress().set_overall(fraction);
            w.progress().set_overall_indeterminate(indeterminate);
        });

        self.poll_tray();
        self.update_tray_tooltip(finished, total, fraction, false);
        if self.progress.as_ref().is_some_and(Progress::is_finished) {
            self.finish();
        }
    }

    fn cancel_job(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
            self.with_window(|w| {
                w.progress().set_cancelling(true);
                w.progress().set_progress_title(shared(strings::CANCELLING));
            });
            if let Some(tray) = &self.tray {
                tray.set_tooltip(&strings::tray_cancelling(APP_DISPLAY_NAME));
            }
        }
    }

    fn is_running(&self) -> bool {
        self.job.is_some()
    }

    fn finish(&mut self) {
        self.timer.stop();
        // The user cancelled (window or tray): they know, no toast.
        let cancelled = self.job.as_ref().is_some_and(|j| j.cancel.is_cancelled());
        if let Some(job) = self.job.take() {
            // JobFinished was the thread's last event; this returns at once.
            job.join();
        }
        let Some(progress) = &self.progress else {
            return;
        };
        // Rebuild the whole list from the final row states (a model reset
        // recreates every list item), so no row can stay on a stale "68%".
        self.rows_model.set_vec(
            progress
                .row_views()
                .into_iter()
                .map(progress_text::row)
                .collect::<Vec<_>>(),
        );
        let summary = progress.summary().cloned().unwrap_or_default();
        let saved = progress.saved(self.job_folder.as_deref());
        let results_id = self.app_data.as_deref().and_then(|dir| {
            let id = results::new_id();
            match saved.save(&paths::results_dir(dir), &id) {
                Ok(()) => Some(id),
                Err(err) => {
                    applog::error("could not save results", &err);
                    None
                }
            }
        });
        self.open_folder = saved.folder.clone();
        self.with_window(|w| {
            w.progress()
                .set_progress_title(SharedString::from(saved.summary.as_str()));
            w.progress().set_overall(1.0);
            w.progress().set_overall_indeterminate(false);
            w.progress().set_finished(true);
            w.progress().set_had_problems(had_problems(&summary));
            w.progress().set_cancelling(false);
            w.progress().set_can_open_folder(saved.folder.is_some());
        });

        if self.settings.output.open_folder_when_done
            && summary.done > 0
            && let Some(folder) = &self.open_folder
            && let Err(err) = shell::open_folder(folder)
        {
            applog::error("could not open the output folder", &err);
        }
        let looking = !self.hidden && focus::app_is_foreground();
        let mut toast_shown = false;
        if looking || cancelled || !self.settings.notifications.toast {
            tracing::info!(
                looking,
                cancelled,
                enabled = self.settings.notifications.toast,
                "no toast"
            );
        } else {
            let toast = toast::Toast {
                title: progress::toast_title(&summary),
                // Second line: where the files went (the summary's subtitle).
                body: (summary.done > 0)
                    .then(|| saved_where(self.job_folder.as_deref()).to_string()),
                open_folder: self.open_folder.clone(),
                results_id,
            };
            match toast::show(&toast) {
                Ok(()) => {
                    toast_shown = true;
                    tracing::info!(hidden = self.hidden, title = %toast.title, "toast shown");
                }
                Err(err) => tracing::warn!("toast not shown: {err:#}"),
            }
        }
        self.tray = None;
        if self.hidden {
            if toast_shown {
                // Show() hands the toast to the notification service over RPC;
                // give it a moment before `hard_exit` kills the process.
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            self.quit();
        }
    }

    // ── Background / tray ────────────────────────────────────────────────────

    /// Hide the window and show the tray icon. `hide_window`: false when the
    /// window is already closing (close button). Returns whether the window
    /// may be hidden: without a tray icon it is minimized instead, so the job
    /// never runs with no way to see or cancel it.
    fn go_background(&mut self, hide_window: bool) -> bool {
        if !self.is_running() {
            return false;
        }
        if self.tray.is_none() {
            match tray::Tray::new(APP_DISPLAY_NAME) {
                Ok(tray) => self.tray = Some(tray),
                Err(err) => applog::error("could not create the tray icon", &err),
            }
        }
        if self.tray.is_none() {
            self.with_window(|w| w.window().set_minimized(true));
            return false;
        }
        if hide_window {
            self.with_window(|w| {
                if let Err(err) = w.hide() {
                    applog::error("could not hide the window", &err);
                }
            });
        }
        self.hidden = true;
        self.last_tray_update = None;
        if let Some(p) = &self.progress {
            let (finished, total, fraction) = (p.finished(), p.total(), p.fraction());
            self.update_tray_tooltip(finished, total, fraction, true);
        }
        true
    }

    fn update_tray_tooltip(&mut self, finished: usize, total: usize, fraction: f32, now: bool) {
        let Some(tray) = &self.tray else {
            return;
        };
        let due = now
            || self
                .last_tray_update
                .is_none_or(|t| t.elapsed() >= TRAY_INTERVAL);
        let cancelling = self.job.as_ref().is_some_and(|j| j.cancel.is_cancelled());
        if due && !cancelling {
            let percent = (fraction * 100.0).round() as u32;
            let mut tip = strings::tray_tooltip(APP_DISPLAY_NAME, finished, total, percent);
            if let Some(status) = self.progress.as_ref().and_then(Progress::lone_status) {
                tip = strings::tray_tooltip_with_status(&tip, status);
            }
            tray.set_tooltip(&tip);
            self.last_tray_update = Some(Instant::now());
        }
    }

    fn poll_tray(&mut self) {
        let mut events = Vec::new();
        if let Some(tray) = &self.tray {
            while let Some(event) = tray.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                tray::TrayEvent::ShowProgress => self.show_again(),
                tray::TrayEvent::Cancel => self.cancel_job(),
            }
        }
    }

    fn show_again(&mut self) {
        self.with_window(|w| {
            if let Err(err) = w.show() {
                applog::error("could not show the window", &err);
            }
        });
        self.hidden = false;
        // The tray icon is only there while the job runs in the background.
        self.tray = None;
    }

    // ── Closing ──────────────────────────────────────────────────────────────

    fn close_requested(&mut self) -> CloseRequestResponse {
        if self.is_running() {
            if !self.go_background(false) {
                return CloseRequestResponse::KeepWindowShown;
            }
        } else {
            self.quit();
        }
        CloseRequestResponse::HideWindow
    }

    fn escape(&mut self) {
        let mut page = PAGE_OPTIONS;
        self.with_window(|w| page = w.get_page());
        if self.is_running() {
            self.go_background(true);
        } else if page == PAGE_HOME {
            self.quit();
        } else if page == PAGE_SETTINGS {
            self.settings_back();
        } else {
            self.close_or_go_home();
        }
    }

    fn quit(&mut self) {
        if let Some(job) = self.job.take() {
            // Only reachable if a job is somehow still running. Cancel and wait,
            // so no half-written temp file is left when the process ends.
            job.cancel.cancel();
            job.join();
        }
        self.tray = None;
        self.timer.stop();
        if let Err(err) = slint::quit_event_loop() {
            applog::error("could not stop the event loop", &err);
        }
    }

    // ── Results ──────────────────────────────────────────────────────────────

    fn outputs_of(&self, index: i32) -> Vec<PathBuf> {
        let Ok(i) = usize::try_from(index) else {
            return Vec::new();
        };
        if let Some(progress) = &self.progress {
            return match progress.rows().get(i).map(|r| &r.state) {
                Some(progress::RowState::Done { outputs, .. }) => outputs.clone(),
                _ => Vec::new(),
            };
        }
        self.saved
            .as_ref()
            .and_then(|s| s.rows.get(i))
            .map(|r| r.outputs.clone())
            .unwrap_or_default()
    }

    fn row_open(&self, index: i32) {
        let result = match progress::open_target(&self.outputs_of(index), Path::is_dir) {
            Some(progress::OpenTarget::File(file)) => shell::open_file(&file),
            Some(progress::OpenTarget::Folder(folder)) => shell::open_folder(&folder),
            None => return,
        };
        if let Err(err) = result {
            applog::error("could not open the result", &err);
        }
    }

    fn row_show(&self, index: i32) {
        let targets = progress::show_targets(&self.outputs_of(index));
        if targets.is_empty() {
            return;
        }
        let refs: Vec<&Path> = targets.iter().map(PathBuf::as_path).collect();
        if let Err(err) = shell::show_in_folder(&refs) {
            applog::error("could not show the file in its folder", &err);
        }
    }

    fn open_results_folder(&self) {
        if let Some(folder) = &self.open_folder
            && let Err(err) = shell::open_folder(folder)
        {
            applog::error("could not open the folder", &err);
        }
    }
}

/// Where request files may come from: the shell DLL writes to the package's
/// `LocalState`; the app's own data folder is the same when packaged.
fn requests_dirs() -> Vec<PathBuf> {
    [paths::shell_data_dir(), paths::app_data_dir()]
        .into_iter()
        .flatten()
        .map(|root| paths::requests_dir(&root))
        .collect()
}
