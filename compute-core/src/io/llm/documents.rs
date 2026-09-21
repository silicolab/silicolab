//! Request-time resolution of attached documents.
//!
//! History holds [`DocumentRef`]s only. Each adapter calls [`resolve`] inside
//! `complete()` (already on a worker thread) to turn them into what its wire
//! format can carry: the file itself, its extracted text, or just its path.
//! Resolution is a pure function of the history and the files on disk, so a
//! replayed request renders the same prefix and provider prompt caches hold.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::UNIX_EPOCH;

use base64::Engine;

use super::types::{ChatMessage, ContentBlock, DocumentRef, LlmError};

/// Only this many of the most recent messages with attachments are sent in
/// full; a native PDF is re-uploaded on every request, so older ones shrink to
/// a pointer the model can follow with `read_pdf`.
pub const FULL_MESSAGE_WINDOW: usize = 2;

const EXTRACT_BUDGET_CHARS: usize = 60_000;

#[derive(Debug, Clone, Copy)]
pub struct NativeLimits {
    /// Raw bytes across one request's PDFs, before base64 inflates them.
    pub max_raw_bytes: u64,
    pub max_pages: Option<u32>,
}

/// The Messages API caps a request at 32 MB and a PDF at 100 pages.
pub const ANTHROPIC_LIMITS: NativeLimits = NativeLimits {
    max_raw_bytes: 22 * 1024 * 1024,
    max_pages: Some(100),
};

/// Chat Completions caps the files of one request at 50 MB.
pub const OPENAI_LIMITS: NativeLimits = NativeLimits {
    max_raw_bytes: 36 * 1024 * 1024,
    max_pages: None,
};

#[derive(Debug, Clone, Copy)]
pub enum DocMode {
    Native(NativeLimits),
    ExtractedText,
    PathMention,
}

/// History with every [`ContentBlock::Document`] either replaced by text or
/// backed by a payload in [`Resolved::payload`].
pub struct Resolved<'a> {
    pub messages: Cow<'a, [ChatMessage]>,
    payloads: HashMap<PathBuf, String>,
}

impl Default for Resolved<'_> {
    fn default() -> Self {
        Self {
            messages: Cow::Borrowed(&[]),
            payloads: HashMap::new(),
        }
    }
}

impl Resolved<'_> {
    /// Base64 of a document that stayed native.
    pub fn payload(&self, document: &DocumentRef) -> Option<&str> {
        self.payloads.get(&document.path).map(String::as_str)
    }
}

pub fn modified_ms(metadata: &Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Validate `path` as a readable PDF and record the facts a later replay
/// compares against.
#[cfg(feature = "pdf")]
pub fn describe(path: &Path) -> Result<DocumentRef, crate::io::pdf::PdfError> {
    let info = crate::io::pdf::probe(path)?;
    let modified_ms = std::fs::metadata(path)
        .as_ref()
        .map(modified_ms)
        .unwrap_or(0);
    Ok(DocumentRef {
        path: path.to_path_buf(),
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        bytes: info.bytes,
        modified_ms,
        pages: info.pages as u32,
    })
}

pub fn resolve<'a>(
    history: &'a [ChatMessage],
    mode: DocMode,
    cancel: &AtomicBool,
) -> Result<Resolved<'a>, LlmError> {
    let with_documents: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, message)| has_document(message))
        .map(|(index, _)| index)
        .collect();
    if with_documents.is_empty() {
        return Ok(Resolved {
            messages: Cow::Borrowed(history),
            payloads: HashMap::new(),
        });
    }
    let first_full = with_documents[with_documents.len().saturating_sub(FULL_MESSAGE_WINDOW)];

    let mut resolver = Resolver {
        mode,
        cancel,
        raw_bytes: 0,
        payloads: HashMap::new(),
    };
    let mut messages = Vec::with_capacity(history.len());
    for (index, message) in history.iter().enumerate() {
        if !has_document(message) {
            messages.push(message.clone());
            continue;
        }
        let documents = message
            .content
            .iter()
            .filter(|block| matches!(block, ContentBlock::Document(_)))
            .count();
        let mut content = Vec::with_capacity(message.content.len());
        for block in &message.content {
            match block {
                ContentBlock::Document(document) if index < first_full => {
                    content.push(ContentBlock::Text(stale_note(document)));
                }
                ContentBlock::Document(document) => {
                    resolver.resolve_document(document, documents, &mut content)?;
                }
                other => content.push(other.clone()),
            }
        }
        messages.push(ChatMessage {
            role: message.role,
            content,
        });
    }
    Ok(Resolved {
        messages: Cow::Owned(messages),
        payloads: resolver.payloads,
    })
}

fn has_document(message: &ChatMessage) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Document(_)))
}

pub fn stale_note(document: &DocumentRef) -> String {
    format!(
        "[earlier attachment: {} at `{}` — call read_pdf to read it again]\n",
        document.name,
        document.path.display()
    )
}

struct Resolver<'c> {
    mode: DocMode,
    cancel: &'c AtomicBool,
    raw_bytes: u64,
    payloads: HashMap<PathBuf, String>,
}

impl Resolver<'_> {
    fn resolve_document(
        &mut self,
        document: &DocumentRef,
        siblings: usize,
        out: &mut Vec<ContentBlock>,
    ) -> Result<(), LlmError> {
        if let DocMode::PathMention = self.mode {
            out.push(ContentBlock::Text(format!(
                "[attached PDF: {} — read it from disk]\n",
                document.path.display()
            )));
            return Ok(());
        }
        let Ok(metadata) = std::fs::metadata(&document.path) else {
            out.push(ContentBlock::Text(format!(
                "[attached PDF {} is no longer available at `{}`]\n",
                document.name,
                document.path.display()
            )));
            return Ok(());
        };
        let changed =
            metadata.len() != document.bytes || modified_ms(&metadata) != document.modified_ms;

        let mut fallback_reason = None;
        if let DocMode::Native(limits) = self.mode {
            match self.load_native(document, &metadata, changed, limits) {
                Ok(()) => {
                    out.push(ContentBlock::Document(document.clone()));
                    if changed {
                        out.push(ContentBlock::Text(format!(
                            "[{} changed on disk after it was attached; this is the current file]\n",
                            document.name
                        )));
                    }
                    return Ok(());
                }
                Err(reason) => fallback_reason = Some(reason),
            }
        }
        let text = self.extracted_text(document, EXTRACT_BUDGET_CHARS / siblings.max(1))?;
        out.push(ContentBlock::Text(match fallback_reason {
            Some(reason) => format!("[{reason}; sending its extracted text instead]\n{text}"),
            None => text,
        }));
        Ok(())
    }

    /// Reads and encodes the file, or says why it cannot go native.
    fn load_native(
        &mut self,
        document: &DocumentRef,
        metadata: &Metadata,
        changed: bool,
        limits: NativeLimits,
    ) -> Result<(), String> {
        let pages = if changed {
            current_pages(&document.path).unwrap_or(document.pages)
        } else {
            document.pages
        };
        if let Some(max_pages) = limits.max_pages
            && pages > max_pages
        {
            return Err(format!(
                "{} has {pages} pages, over the provider's {max_pages}-page PDF limit",
                document.name
            ));
        }
        if self.payloads.contains_key(&document.path) {
            return Ok(());
        }
        if self.raw_bytes + metadata.len() > limits.max_raw_bytes {
            return Err(format!(
                "{} would push this request's PDFs past the provider's size limit",
                document.name
            ));
        }
        let bytes = std::fs::read(&document.path)
            .map_err(|error| format!("{} could not be read ({error})", document.name))?;
        self.raw_bytes += bytes.len() as u64;
        self.payloads.insert(
            document.path.clone(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
        Ok(())
    }

    #[cfg(feature = "pdf")]
    fn extracted_text(&self, document: &DocumentRef, budget: usize) -> Result<String, LlmError> {
        use crate::io::pdf::{PdfError, extract_pages, render_paged};
        match extract_pages(&document.path, None, self.cancel) {
            Ok((info, pages)) => Ok(format!(
                "[attached PDF: {} at `{}` — extracted text follows; call read_pdf for other \
                 pages or to search it]\n{}\n\n",
                document.name,
                document.path.display(),
                render_paged(&info, &pages, budget)
            )),
            Err(PdfError::Cancelled) => Err(LlmError::Cancelled),
            Err(error) => Ok(format!(
                "[attached PDF {} could not be read: {error}]\n",
                document.name
            )),
        }
    }

    #[cfg(not(feature = "pdf"))]
    fn extracted_text(&self, document: &DocumentRef, _budget: usize) -> Result<String, LlmError> {
        let _ = self.cancel;
        Ok(format!(
            "[attached PDF: {} — read it from disk]\n",
            document.path.display()
        ))
    }
}

#[cfg(feature = "pdf")]
fn current_pages(path: &Path) -> Option<u32> {
    crate::io::pdf::probe(path)
        .ok()
        .map(|info| info.pages as u32)
}

#[cfg(not(feature = "pdf"))]
fn current_pages(_path: &Path) -> Option<u32> {
    None
}

#[cfg(all(test, feature = "pdf"))]
mod tests;
