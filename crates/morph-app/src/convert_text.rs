// SPDX-License-Identifier: GPL-3.0-or-later
//! Puts the conversion window's text (`Strings` global) and its fixed lists
//! (format boxes) into the window. The text itself lives in `strings/`.

use crate::controller::Views;
use crate::media_options::{self, VideoKind};
use crate::office_options::OfficeKind;
use crate::options::ImageKind;
use crate::{AudioFormatInfo, MainWindow, Strings, strings};
use morph_core::media::options::AudioFormat;
use morph_core::office::options::DocImageFormat;
use morph_core::pdf::options::{DPI_PRESETS, PageImageFormat};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

fn shared(text: &str) -> SharedString {
    SharedString::from(text)
}

fn list(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.iter().map(|s| shared(s)).collect::<Vec<_>>(),
    ))
}

fn list_owned(items: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.iter().map(|s| shared(s)).collect::<Vec<_>>(),
    ))
}

/// Set every text of the conversion window. Called once at startup.
pub fn apply(window: &MainWindow) {
    use strings as s;
    let g = window.global::<Strings>();
    g.set_cancel(shared(s::CANCEL));
    g.set_show_files(shared(s::SHOW_FILES));
    g.set_hide_files(shared(s::HIDE_FILES));
    g.set_remove_file(shared(s::REMOVE_FILE));
    let kinds: Vec<&str> = ImageKind::SELECTOR.iter().map(|k| k.label()).collect();
    g.set_actions(list(&kinds));
    g.set_format(shared(s::FORMAT));
    g.set_quality(shared(s::QUALITY));
    g.set_quality_default(shared(s::QUALITY_DEFAULT));
    g.set_quality_compress_hint(shared(s::QUALITY_COMPRESS_HINT));
    g.set_lossless(shared(s::LOSSLESS));
    g.set_keep_metadata(shared(s::KEEP_METADATA));
    g.set_remove_gps(shared(s::REMOVE_GPS));
    g.set_background(shared(s::BACKGROUND));
    g.set_backgrounds(list(&[s::BACKGROUND_WHITE, s::BACKGROUND_BLACK]));
    g.set_ico_sizes(shared(s::ICO_SIZES));
    g.set_svg_width(shared(s::SVG_WIDTH));
    g.set_compress_mode(shared(s::COMPRESS_MODE));
    g.set_compress_best(shared(s::COMPRESS_BEST));
    g.set_compress_best_hint(shared(s::COMPRESS_BEST_HINT));
    g.set_compress_by_quality(shared(s::COMPRESS_BY_QUALITY));
    g.set_compress_by_size(shared(s::COMPRESS_BY_SIZE));
    g.set_target_at_most(shared(s::TARGET_SIZE_AT_MOST));
    g.set_units(list(&[s::UNIT_KB, s::UNIT_MB]));
    g.set_convert_to_webp(shared(s::CONVERT_TO_WEBP));
    g.set_png_lossy(shared(s::PNG_LOSSY));
    g.set_resize_mode(shared(s::RESIZE_MODE));
    g.set_resize_modes(list(&[
        s::RESIZE_PERCENT,
        s::RESIZE_MAX_WIDTH,
        s::RESIZE_MAX_HEIGHT,
        s::RESIZE_EXACT,
    ]));
    g.set_percent(shared(s::PERCENT));
    g.set_width(shared(s::WIDTH));
    g.set_height(shared(s::HEIGHT));
    g.set_keep_aspect(shared(s::KEEP_ASPECT));
    g.set_no_enlarge(shared(s::NO_ENLARGE));
    g.set_strip_label(shared(s::STRIP_LABEL));
    g.set_strips(list(&[s::STRIP_ALL, s::STRIP_GPS, s::STRIP_NONE]));
    g.set_rotate(shared(s::ROTATE));
    g.set_rotations(list(&[
        s::ROTATE_NONE,
        s::ROTATE_90,
        s::ROTATE_180,
        s::ROTATE_270,
    ]));
    g.set_flip_h(shared(s::FLIP_H));
    g.set_flip_v(shared(s::FLIP_V));
    g.set_clean_rotate_note(shared(s::CLEAN_ROTATE_NOTE));
    g.set_pdf_compression(shared(s::PDF_COMPRESSION));
    g.set_pdf_presets(list(&[
        s::PDF_PRESET_LOW,
        s::PDF_PRESET_MEDIUM,
        s::PDF_PRESET_HIGH,
        s::PDF_PRESET_CUSTOM,
    ]));
    g.set_pdf_preset_hints(list(&s::PDF_PRESET_HINTS));
    g.set_pdf_max_dpi(shared(s::PDF_MAX_DPI));
    g.set_pdf_compress_note(shared(s::PDF_COMPRESS_NOTE));
    let page_formats: Vec<&str> = PageImageFormat::ALL
        .iter()
        .map(|f| s::format_label(f.output_format()))
        .collect();
    g.set_pdf_image_formats(list(&page_formats));
    g.set_pdf_dpi(shared(s::PDF_DPI));
    let mut dpis: Vec<SharedString> = DPI_PRESETS
        .iter()
        .map(|d| SharedString::from(s::dpi_label(*d)))
        .collect();
    dpis.push(shared(s::PDF_DPI_CUSTOM));
    g.set_pdf_dpis(ModelRc::new(VecModel::from(dpis)));
    g.set_pdf_dpi_value(shared(s::PDF_DPI_VALUE));
    g.set_pdf_pages(shared(s::PDF_PAGES));
    g.set_pdf_pages_placeholder(shared(s::PDF_PAGES_PLACEHOLDER));
    g.set_pdf_to_images_note(shared(s::PDF_TO_IMAGES_NOTE));
    g.set_pdf_page_separators(shared(s::PDF_PAGE_SEPARATORS));
    g.set_pdf_to_text_note(shared(s::PDF_TO_TEXT_NOTE));
    g.set_pdf_split_mode(shared(s::PDF_SPLIT_MODE));
    g.set_pdf_split_modes(list(&[
        s::PDF_SPLIT_EVERY_PAGE,
        s::PDF_SPLIT_EVERY_N,
        s::PDF_SPLIT_RANGES,
    ]));
    g.set_pdf_split_every(shared(s::PDF_SPLIT_EVERY));
    g.set_pdf_split_ranges(shared(s::PDF_SPLIT_RANGES_LABEL));
    g.set_pdf_split_ranges_placeholder(shared(s::PDF_SPLIT_RANGES_PLACEHOLDER));
    g.set_pdf_split_note(shared(s::PDF_SPLIT_NOTE));
    g.set_pdf_page_size(shared(s::PDF_PAGE_SIZE));
    g.set_pdf_page_sizes(list(&[s::PDF_PAGE_FIT, s::PDF_PAGE_A4, s::PDF_PAGE_LETTER]));
    g.set_pdf_margin(shared(s::PDF_MARGIN));
    g.set_pdf_bookmarks(shared(s::PDF_BOOKMARKS));
    g.set_pdf_merge_note(shared(s::PDF_MERGE_NOTE));
    g.set_merge_order_hint(shared(s::MERGE_ORDER_HINT));
    g.set_pdf_tabs(list(&s::PDF_TABS));
    g.set_pdf_convert_to(shared(s::PDF_CONVERT_TO));
    g.set_pdf_convert_chips(list(&s::PDF_CONVERT_CHIPS));
    g.set_pdf_preset_descs(list(&s::PDF_PRESET_DESCS));
    g.set_compress_mode_descs(list(&s::COMPRESS_MODE_DESCS));
    g.set_chip_pdf(shared(s::CHIP_PDF));
    g.set_chip_ico(shared(s::CHIP_ICO));
    g.set_move_up(shared(s::MOVE_UP));
    g.set_move_down(shared(s::MOVE_DOWN));
    g.set_save_to(shared(s::SAVE_TO));
    g.set_save_next_to(shared(s::SAVE_NEXT_TO));
    g.set_save_choose(shared(s::SAVE_CHOOSE));
    g.set_save_browse(shared(s::SAVE_BROWSE));
    g.set_save_no_folder(shared(s::SAVE_NO_FOLDER));
    g.set_remember(shared(s::REMEMBER));
    g.set_pdf_to_word_warning(shared(s::PDF_TO_WORD_WARNING));
    g.set_pdf_to_word_note(shared(s::PDF_TO_WORD_NOTE));
    g.set_pdf_to_markdown_note(shared(s::PDF_TO_MARKDOWN_NOTE));
    g.set_save_pictures(shared(s::SAVE_PICTURES));
    g.set_image_to_pdf_note(shared(s::IMAGE_TO_PDF_NOTE));
    g.set_max_rows(shared(s::MAX_ROWS));
    g.set_slide_notes(shared(s::SLIDE_NOTES));
    let office_kinds: Vec<&str> = OfficeKind::ALL.iter().map(|k| k.label()).collect();
    g.set_office_actions(list(&office_kinds));
    g.set_office_convert_to(shared(s::OFFICE_CONVERT_TO));
    let doc_images: Vec<&str> = DocImageFormat::ALL
        .iter()
        .map(|f| s::format_label(f.output_format()))
        .collect();
    g.set_office_image_formats(list(&doc_images));
    g.set_csv_delimiter(shared(s::CSV_DELIMITER));
    g.set_csv_delimiters(list(&s::CSV_DELIMITERS));
    g.set_csv_delimiter_hint(shared(s::CSV_DELIMITER_HINT));
    g.set_csv_bom(shared(s::CSV_BOM));
    g.set_office_convert_note(shared(s::OFFICE_CONVERT_NOTE));
    g.set_office_images_note(shared(s::OFFICE_IMAGES_NOTE));
    g.set_office_images_only(shared(s::OFFICE_IMAGES_ONLY));
    g.set_engine_missing(shared(s::ENGINE_MISSING));
    g.set_download_libreoffice(shared(s::DOWNLOAD_LIBREOFFICE));
    g.set_check_again(shared(s::CHECK_AGAIN));
    g.set_settings(shared(s::SETTINGS_BUTTON));
    media(window);
}

fn media(window: &MainWindow) {
    use morph_core::media::options::{GIF_FPS, GIF_WIDTHS};
    use strings as s;
    let g = window.global::<Strings>();
    let kinds: Vec<&str> = VideoKind::ALL.iter().map(|k| k.label()).collect();
    g.set_video_actions(list(&kinds));
    g.set_advanced_show(shared(s::ADVANCED_SHOW));
    g.set_advanced_hide(shared(s::ADVANCED_HIDE));
    g.set_video_containers(list(&s::VIDEO_CONTAINERS));
    g.set_video_fast(shared(s::VIDEO_FAST));
    g.set_video_fast_hint(shared(s::VIDEO_FAST_HINT));
    g.set_video_codec(shared(s::VIDEO_CODEC));
    g.set_video_codecs(list(&s::VIDEO_CODECS));
    g.set_audio_codec(shared(s::AUDIO_CODEC));
    g.set_video_audio_codecs(list(&s::VIDEO_AUDIO_CODECS));
    g.set_video_quality(shared(s::VIDEO_QUALITY));
    g.set_video_qualities(list(&s::VIDEO_QUALITIES));
    g.set_video_encoder(shared(s::VIDEO_ENCODER));
    g.set_video_encoders(list(&s::VIDEO_ENCODERS));
    g.set_video_convert_note(shared(s::VIDEO_CONVERT_NOTE));
    g.set_video_preset(shared(s::VIDEO_PRESET));
    g.set_video_presets(list(&s::VIDEO_PRESETS));
    g.set_video_preset_hints(list(&s::VIDEO_PRESET_HINTS));
    g.set_video_target_mb(shared(s::VIDEO_TARGET_MB));
    g.set_video_resolution(shared(s::VIDEO_RESOLUTION));
    g.set_video_resolutions(list(&s::VIDEO_RESOLUTIONS));
    g.set_video_resolution_hint(shared(s::VIDEO_RESOLUTION_HINT));
    g.set_extract_audio_note(shared(s::EXTRACT_AUDIO_NOTE));
    g.set_gif_start(shared(s::GIF_START));
    g.set_gif_start_placeholder(shared(s::GIF_START_PLACEHOLDER));
    g.set_gif_length(shared(s::GIF_LENGTH));
    g.set_gif_width(shared(s::GIF_WIDTH));
    let widths: Vec<String> = GIF_WIDTHS.iter().map(|w| s::gif_width_label(*w)).collect();
    g.set_gif_widths(list_owned(&widths));
    g.set_gif_fps(shared(s::GIF_FPS));
    let fps: Vec<String> = GIF_FPS.iter().map(|f| s::gif_fps_label(*f)).collect();
    g.set_gif_fps_choices(list_owned(&fps));
    g.set_gif_note(shared(s::GIF_NOTE));
    g.set_audio_bitrate(shared(s::AUDIO_BITRATE));
    g.set_mp3_mode(shared(s::MP3_MODE));
    g.set_mp3_modes(list(&s::MP3_MODES));
    g.set_sample_rate(shared(s::SAMPLE_RATE));
    g.set_sample_rates(list(&s::SAMPLE_RATES));
    g.set_channels(shared(s::CHANNELS));
    g.set_channel_choices(list(&s::CHANNEL_CHOICES));
    g.set_keep_tags(shared(s::KEEP_TAGS));
    g.set_audio_convert_note(shared(s::AUDIO_CONVERT_NOTE));
    g.set_lossless_warning(shared(s::LOSSLESS_WARNING));
    g.set_ffmpeg_missing(shared(s::FFMPEG_MISSING));

    let infos = |formats: &[AudioFormat]| {
        let infos: Vec<AudioFormatInfo> = formats
            .iter()
            .map(|f| {
                let rates: Vec<String> = f.bitrates().iter().map(|b| s::kbps_label(*b)).collect();
                AudioFormatInfo {
                    label: shared(s::audio_format_label(*f)),
                    lossless: f.is_lossless(),
                    is_mp3: *f == AudioFormat::Mp3,
                    bitrates: list_owned(&rates),
                    default_bitrate: f
                        .bitrates()
                        .iter()
                        .position(|b| *b == f.default_bitrate())
                        .and_then(|i| i32::try_from(i).ok())
                        .unwrap_or(0),
                }
            })
            .collect();
        let labels: Vec<&str> = formats.iter().map(|f| s::audio_format_label(*f)).collect();
        (ModelRc::new(VecModel::from(infos)), list(&labels))
    };
    let (formats, labels) = infos(&media_options::EXTRACT_FORMATS);
    window.convert().set_extract_formats(formats);
    window.convert().set_extract_format_labels(labels);
    let (formats, labels) = infos(&media_options::AUDIO_FORMATS);
    window.convert().set_audio_formats(formats);
    window.convert().set_audio_format_labels(labels);
}
