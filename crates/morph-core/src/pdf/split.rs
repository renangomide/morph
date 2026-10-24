// SPDX-License-Identifier: GPL-3.0-or-later
//! Split (SPEC §4.2): every page / every N pages / custom ranges into
//! `name (split)/name-p1-3.pdf` (`name-p7.pdf` for a one-page part).
//!
//! Each part is a new PDFium document with the pages imported from the input
//! (`FPDF_ImportPagesByIndex`), saved with [`pdfium::save`] into an
//! [`OutputDir`]. Custom ranges are written in the order typed and may overlap
//! (`1-3,2-4` gives two files sharing pages 2-3). On any error or cancel the
//! output folder is removed, so a failed split leaves nothing behind.
//!
//! `TaskOutput.outputs` lists the part files in order, like PDF → images (so
//! sizes add up; the results row opens their shared `name (split)` folder).

use super::options::{PdfSplitOpts, SplitMode};
use super::pages::{self, PageRange};
use super::pdfium;
use crate::action::Action;
use crate::converter::{Converter, TaskCtx, TaskOutput};
use crate::error::ConvertError;
use crate::job::Task;
use crate::naming::{MultiOutput, OutputDir, multi_output_dir_name, split_file_name};
use pdfium_render::prelude::PdfPageIndex;

pub struct PdfSplit;

/// The parts to write, 1-based, for a document with `count` pages.
pub fn parts(opts: &PdfSplitOpts, count: u32) -> Result<Vec<PageRange>, ConvertError> {
    if count == 0 {
        return Err(ConvertError::unsupported(
            "This PDF has no pages.",
            "page count 0",
        ));
    }
    match opts.mode {
        SplitMode::EveryPage => Ok(pages::chunks(count, 1)),
        SplitMode::EveryN => Ok(pages::chunks(count, opts.every.max(1))),
        SplitMode::Ranges => pages::parse_ranges(&opts.ranges, count),
    }
}

fn index(page: u32) -> Result<PdfPageIndex, ConvertError> {
    PdfPageIndex::try_from(page)
        .map_err(|e| ConvertError::other("The PDF engine failed on this file.", e))
}

impl Converter for PdfSplit {
    fn run(&self, task: &Task, ctx: &TaskCtx<'_>) -> Result<TaskOutput, ConvertError> {
        let Action::PdfSplit(opts) = &task.action else {
            return Err(super::wrong_action("PdfSplit"));
        };
        let input = task
            .input()
            .ok_or_else(|| ConvertError::other("No file to split.", "task has no input"))?;
        let src = pdfium::open(input)?;
        let count = pdfium::page_count(&src);
        // Validate the ranges before creating the folder: a typo leaves nothing behind.
        let parts = parts(opts, count)?;
        let folder = task.output.folder_for(input);
        let dir = OutputDir::create(&folder, &multi_output_dir_name(input, MultiOutput::Split))?;
        let total = parts.len();
        let mut outputs = Vec::with_capacity(total);
        for (done, part) in parts.iter().enumerate() {
            ctx.check_cancel()?;
            let mut doc = pdfium::new_document()?;
            doc.pages_mut()
                .copy_page_range_from_document(
                    &src,
                    index(part.first - 1)?..=index(part.last - 1)?,
                    0,
                )
                .map_err(|e| pdfium::map_error(e, &format!("copy pages {}", part.label())))?;
            outputs.push(pdfium::save(
                &doc,
                dir.path(),
                &split_file_name(input, part.first, part.last),
            )?);
            ctx.progress((done + 1) as f32 / total as f32, None);
        }
        dir.commit();
        Ok(TaskOutput {
            outputs,
            note: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(mode: SplitMode, every: u32, ranges: &str) -> PdfSplitOpts {
        PdfSplitOpts {
            mode,
            every,
            ranges: ranges.to_owned(),
        }
    }

    fn labels(p: &[PageRange]) -> Vec<String> {
        p.iter().map(PageRange::label).collect()
    }

    #[test]
    fn parts_per_mode() {
        let every = parts(&opts(SplitMode::EveryPage, 0, ""), 3).unwrap();
        assert_eq!(labels(&every), ["1", "2", "3"]);
        let by2 = parts(&opts(SplitMode::EveryN, 2, ""), 5).unwrap();
        assert_eq!(labels(&by2), ["1-2", "3-4", "5"]);
        let zero = parts(&opts(SplitMode::EveryN, 0, ""), 2).unwrap();
        assert_eq!(labels(&zero), ["1", "2"]);
        let custom = parts(&opts(SplitMode::Ranges, 0, " 1-3, 4 -10 ,11- "), 12).unwrap();
        assert_eq!(labels(&custom), ["1-3", "4-10", "11-12"]);
        let overlap = parts(&opts(SplitMode::Ranges, 0, "1-3,2-4"), 5).unwrap();
        assert_eq!(labels(&overlap), ["1-3", "2-4"]);
    }

    #[test]
    fn bad_ranges_are_plain_english() {
        let err = parts(&opts(SplitMode::Ranges, 0, "4-6"), 3).unwrap_err();
        assert!(err.message.contains("only 3 pages"), "{err}");
        let err = parts(&opts(SplitMode::Ranges, 0, "3-1"), 3).unwrap_err();
        assert!(err.message.contains("isn't valid"), "{err}");
        assert!(parts(&opts(SplitMode::EveryPage, 0, ""), 0).is_err());
    }
}
