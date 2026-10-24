// SPDX-License-Identifier: GPL-3.0-or-later
//! Image → PDF, one PDF per image (`image.to_pdf`, SPEC §4.6). Output
//! `name.pdf` next to the image (collisions get ` (1)`). The page is built by
//! Merge's `add_image_page` (JPEG passthrough, EXIF orientation, lossless for
//! PNG & co.); combining several images into one PDF is `pdf.merge`.

use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::{self, OutputFile, Suffix};
use crate::pdf::{merge, pdfium};

pub struct ImageToPdf;

impl Converter for ImageToPdf {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::ImageToPdf(opts) = &task.action else {
            return Err(crate::pdf::wrong_action("ImageToPdf"));
        };
        let input = task
            .input()
            .ok_or_else(|| ConvertError::other("No image to convert.", "task has no input"))?;
        let mut doc = pdfium::new_document()?;
        merge::add_image_page(&mut doc, input, opts.page_size, opts.margin_mm)?;
        ctx.check_cancel()?;
        let bytes = doc
            .save_to_bytes()
            .map_err(|e| pdfium::map_error(e, "save document"))?;
        drop(doc);
        ctx.check_cancel()?;
        let name = naming::output_name(input, "pdf", Suffix::Converted);
        let mut out = OutputFile::create(&task.output.folder_for(input), &name)?;
        out.write_all(&bytes)?;
        let path = out.commit()?;
        ctx.progress(1.0, None);
        Ok(TaskOutput::single(path))
    }
}
