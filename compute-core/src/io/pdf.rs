//! PDF text extraction for the assistant: page-addressed plain text and keyword
//! search over a document's text layer. Scanned (image-only) PDFs yield empty
//! pages; there is no OCR.

use std::fmt;
use std::ops::RangeInclusive;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use pdf_extract::{Document, PlainTextOutput, output_doc_page};

pub const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PDF_PAGES: usize = 2000;
pub const MAX_PAGES_PER_CALL: u32 = 50;
pub const MAX_SEARCH_HITS: usize = 20;

const SNIPPET_RADIUS: usize = 80;
/// Deeply nested page trees and content streams recurse in the parser; the
/// default 2 MiB of a spawned thread is not enough for some real documents.
const PARSER_STACK_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    NotFound(PathBuf),
    NotAPdf,
    TooLarge { bytes: u64, limit: u64 },
    TooManyPages { pages: usize, limit: usize },
    Encrypted,
    Malformed(String),
    PageOutOfRange { requested: u32, pages: usize },
    BadPageSpec(String),
    Io(String),
    Cancelled,
}

impl fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfError::NotFound(path) => write!(formatter, "no file at {}", path.display()),
            PdfError::NotAPdf => formatter.write_str("the file is not a PDF"),
            PdfError::TooLarge { bytes, limit } => write!(
                formatter,
                "the PDF is {} MB; the limit is {} MB",
                bytes / (1024 * 1024),
                limit / (1024 * 1024)
            ),
            PdfError::TooManyPages { pages, limit } => {
                write!(formatter, "the PDF has {pages} pages; the limit is {limit}")
            }
            PdfError::Encrypted => {
                formatter.write_str("the PDF is password-protected and cannot be read")
            }
            PdfError::Malformed(detail) => write!(formatter, "the PDF is malformed: {detail}"),
            PdfError::PageOutOfRange { requested, pages } => write!(
                formatter,
                "page {requested} is out of range; the PDF has {pages} pages"
            ),
            PdfError::BadPageSpec(spec) => write!(
                formatter,
                "`{spec}` is not a page selection; use a page like `3` or a range like `3-7`"
            ),
            PdfError::Io(detail) => write!(formatter, "could not read the PDF: {detail}"),
            PdfError::Cancelled => formatter.write_str("cancelled"),
        }
    }
}

impl std::error::Error for PdfError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfInfo {
    pub pages: usize,
    pub bytes: u64,
}

/// One page of extracted text. `page` is 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageText {
    pub page: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfHit {
    pub page: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    max_bytes: u64,
    max_pages: usize,
}

const DEFAULT_LIMITS: Limits = Limits {
    max_bytes: MAX_PDF_BYTES,
    max_pages: MAX_PDF_PAGES,
};

pub fn probe(path: &Path) -> Result<PdfInfo, PdfError> {
    guarded(|| load(path, DEFAULT_LIMITS).map(|(info, _)| info))
}

/// Extract a page range (default: from page 1), capped at
/// [`MAX_PAGES_PER_CALL`] pages so one call does bounded work.
pub fn extract_pages(
    path: &Path,
    range: Option<RangeInclusive<u32>>,
    cancel: &AtomicBool,
) -> Result<(PdfInfo, Vec<PageText>), PdfError> {
    guarded(|| extract_pages_with(path, range, cancel, DEFAULT_LIMITS))
}

/// Case-insensitive search across every page, returning up to
/// [`MAX_SEARCH_HITS`] hits with surrounding context.
pub fn search(
    path: &Path,
    query: &str,
    cancel: &AtomicBool,
) -> Result<(PdfInfo, Vec<PdfHit>), PdfError> {
    guarded(|| {
        let (info, document) = load(path, DEFAULT_LIMITS)?;
        let needle = query.trim().to_lowercase();
        let mut hits = Vec::new();
        if needle.is_empty() {
            return Ok((info, hits));
        }
        for page in 1..=info.pages as u32 {
            if cancel.load(Ordering::Relaxed) {
                return Err(PdfError::Cancelled);
            }
            let text = page_text(&document, page);
            collect_hits(page, &text, &needle, &mut hits);
            if hits.len() >= MAX_SEARCH_HITS {
                hits.truncate(MAX_SEARCH_HITS);
                break;
            }
        }
        Ok((info, hits))
    })
}

/// Parse `"3"` or `"3-7"` (1-based, inclusive).
pub fn parse_page_spec(spec: &str) -> Result<RangeInclusive<u32>, PdfError> {
    let bad = || PdfError::BadPageSpec(spec.to_string());
    let parse = |part: &str| part.trim().parse::<u32>().ok().filter(|page| *page >= 1);
    let range = match spec.split_once('-') {
        Some((first, last)) => parse(first).zip(parse(last)).map(|(a, b)| a..=b),
        None => parse(spec).map(|page| page..=page),
    };
    range.filter(|range| !range.is_empty()).ok_or_else(bad)
}

/// Render pages under `[page N]` headers within `budget_chars`, stopping at a
/// page boundary and telling the reader which `pages=` selection continues.
pub fn render_paged(info: &PdfInfo, pages: &[PageText], budget_chars: usize) -> String {
    const FOOTER_RESERVE: usize = 160;
    let Some(first) = pages.first() else {
        return format!("{} pages total; no pages selected.", info.pages);
    };
    let body_budget = budget_chars.saturating_sub(FOOTER_RESERVE);
    let mut out = format!("{} pages total.\n", info.pages);
    let mut used = out.chars().count();
    let mut last_shown = None;
    let mut cut_page = None;
    for page in pages {
        let block = format!("\n[page {}]\n{}\n", page.page, page.text);
        let block_chars = block.chars().count();
        if used + block_chars > body_budget {
            if last_shown.is_none() {
                out.extend(block.chars().take(body_budget.saturating_sub(used)));
                last_shown = Some(page.page);
                cut_page = Some(page.page);
            }
            break;
        }
        out.push_str(&block);
        used += block_chars;
        last_shown = Some(page.page);
    }
    let last_shown = last_shown.unwrap_or(first.page);
    if pages.iter().all(|page| page.text.is_empty()) {
        out.push_str("\nNo text layer on these pages (scanned PDF?); OCR is not supported.\n");
    }
    if let Some(page) = cut_page {
        out.push_str(&format!(
            "\n[page {page} is longer than one result and was cut; use `query` to find the part you need]\n"
        ));
    }
    if (last_shown as usize) < info.pages {
        let next = last_shown + 1;
        let end = (next + MAX_PAGES_PER_CALL - 1).min(info.pages as u32);
        out.push_str(&format!(
            "\nShown through page {last_shown}. Continue with pages={next}-{end}.\n"
        ));
    }
    out
}

pub fn render_hits(info: &PdfInfo, query: &str, hits: &[PdfHit]) -> String {
    if hits.is_empty() {
        return format!("{} pages total. No match for `{query}`.", info.pages);
    }
    let mut out = format!(
        "{} pages total. {} match(es) for `{query}`:\n",
        info.pages,
        hits.len()
    );
    for hit in hits {
        out.push_str(&format!("\n[page {}] …{}…\n", hit.page, hit.snippet));
    }
    if hits.len() == MAX_SEARCH_HITS {
        out.push_str("\nMore matches may exist; narrow the query.\n");
    }
    out
}

fn extract_pages_with(
    path: &Path,
    range: Option<RangeInclusive<u32>>,
    cancel: &AtomicBool,
    limits: Limits,
) -> Result<(PdfInfo, Vec<PageText>), PdfError> {
    let (info, document) = load(path, limits)?;
    let start = range.as_ref().map_or(1, |range| *range.start());
    if start as usize > info.pages {
        return Err(PdfError::PageOutOfRange {
            requested: start,
            pages: info.pages,
        });
    }
    let requested_end = range.map_or(u32::MAX, |range| *range.end());
    let end = requested_end
        .min(info.pages as u32)
        .min(start.saturating_add(MAX_PAGES_PER_CALL - 1));
    let mut pages = Vec::new();
    for page in start..=end {
        if cancel.load(Ordering::Relaxed) {
            return Err(PdfError::Cancelled);
        }
        pages.push(PageText {
            page,
            text: page_text(&document, page),
        });
    }
    Ok((info, pages))
}

fn load(path: &Path, limits: Limits) -> Result<(PdfInfo, Document), PdfError> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => PdfError::NotFound(path.to_path_buf()),
        _ => PdfError::Io(error.to_string()),
    })?;
    if !metadata.is_file() {
        return Err(PdfError::NotAPdf);
    }
    let bytes = metadata.len();
    if bytes > limits.max_bytes {
        return Err(PdfError::TooLarge {
            bytes,
            limit: limits.max_bytes,
        });
    }
    let buffer = std::fs::read(path).map_err(|error| PdfError::Io(error.to_string()))?;
    // The header may be preceded by a few junk bytes in files found in the wild.
    let head = &buffer[..buffer.len().min(1024)];
    if !head.windows(5).any(|window| window == b"%PDF-") {
        return Err(PdfError::NotAPdf);
    }
    let mut document =
        Document::load_mem(&buffer).map_err(|error| PdfError::Malformed(error.to_string()))?;
    // Owner-password-only PDFs open with the empty user password.
    if document.is_encrypted() && document.decrypt("").is_err() {
        return Err(PdfError::Encrypted);
    }
    let pages = document.get_pages().len();
    if pages > limits.max_pages {
        return Err(PdfError::TooManyPages {
            pages,
            limit: limits.max_pages,
        });
    }
    Ok((PdfInfo { pages, bytes }, document))
}

/// Text of one page, or empty when neither extractor can decode it. A single
/// undecodable page must not fail the whole document.
fn page_text(document: &Document, page: u32) -> String {
    let primary = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut text = String::new();
        let mut output = PlainTextOutput::new(&mut text);
        output_doc_page(document, &mut output, page).map(|()| text)
    }));
    let text = match primary {
        Ok(Ok(text)) => text,
        _ => panic::catch_unwind(AssertUnwindSafe(|| document.extract_text(&[page])))
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
    };
    normalize_whitespace(&text)
}

fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank_run > 0 { "\n\n" } else { "\n" });
        }
        blank_run = 0;
        out.push_str(&line);
    }
    out
}

fn collect_hits(page: u32, text: &str, needle: &str, hits: &mut Vec<PdfHit>) {
    let flat = text.replace('\n', " ");
    let chars: Vec<char> = flat.chars().collect();
    let lower: Vec<char> = flat.to_lowercase().chars().collect();
    // Lowercasing can change the char count (e.g. `İ`); fall back to matching
    // the original so indices stay aligned.
    let haystack = if lower.len() == chars.len() {
        &lower
    } else {
        &chars
    };
    let needle: Vec<char> = needle.chars().collect();
    let mut from = 0;
    while let Some(offset) = haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
    {
        let at = from + offset;
        let start = at.saturating_sub(SNIPPET_RADIUS);
        let end = (at + needle.len() + SNIPPET_RADIUS).min(chars.len());
        hits.push(PdfHit {
            page,
            snippet: chars[start..end].iter().collect(),
        });
        if hits.len() >= MAX_SEARCH_HITS {
            return;
        }
        // Skip past this snippet so one dense paragraph does not use every hit.
        from = end;
        if from + needle.len() > haystack.len() {
            return;
        }
    }
}

/// Run parser work on a large-stack thread and turn a panic into an error: the
/// PDF crates index and unwrap freely on malformed input.
fn guarded<T: Send>(work: impl FnOnce() -> Result<T, PdfError> + Send) -> Result<T, PdfError> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("pdf-extract".to_string())
            .stack_size(PARSER_STACK_BYTES)
            .spawn_scoped(scope, work)
            .map_err(|error| PdfError::Io(error.to_string()))?
            .join()
            .unwrap_or_else(|_| {
                Err(PdfError::Malformed(
                    "the parser failed on this file".to_string(),
                ))
            })
    })
}

#[cfg(test)]
pub(crate) mod tests;
