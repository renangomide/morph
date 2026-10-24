// SPDX-License-Identifier: GPL-3.0-or-later
//! Video → Extract audio (SPEC §4.3). The first audio track becomes
//! `clip.mp3` (or the chosen format) next to the video. Stream copy when the
//! source codec already is the target (AAC → M4A, MP3 → MP3, Opus → Opus,
//! Vorbis → OGG, FLAC → FLAC, PCM → WAV) and sample rate/channels are Keep;
//! else the same encode as Audio → Convert ([`super::audio`]).

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;

/// `video.extract_audio`.
pub struct ExtractAudio;

impl Converter for ExtractAudio {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::VideoExtractAudio(opts) = &task.action else {
            return Err(super::wrong_action("ExtractAudio"));
        };
        let (input, info) = super::open(task, ctx)?;
        let stream = info.audio().ok_or_else(super::no_audio_track)?;
        super::audio::convert_audio(input, &info, stream, opts, true, task, ctx)
    }
}
