//! Argument handling for the `read_pdf` tool. The read itself runs off-thread
//! (`jobs::spawn_pdf_read`); this decides what it reads and whether that needs
//! the user's approval.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::frontend::jobs::PdfReadRequest;
use crate::io::pdf::parse_page_spec;

/// Text returned per call. Delivered as a follow-up message rather than a tool
/// result, so it is not subject to the tool-result clamp; sized at roughly 4k
/// tokens so paging through a paper stays cheap to replay.
const READ_BUDGET_CHARS: usize = 16_000;

/// Whether the call's `path` may leave the project folder. Judged from the path
/// text alone so approval gating needs no workspace access: only a path made
/// purely of plain names is certain to stay inside. `is_relative` is not that
/// test — on Windows a rooted path with no drive (`\Windows\x.pdf`, `/x.pdf`)
/// is "relative" yet escapes the project.
pub fn reads_outside_project(input: &Value) -> bool {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return false;
    };
    !Path::new(path)
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

pub fn parse_request(input: &Value, project_root: Option<&Path>) -> Result<PdfReadRequest, String> {
    let raw = input
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or("read_pdf requires a `path` string.")?;
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() || path.has_root() {
        path
    } else {
        project_root
            .ok_or("No project is open, so a relative path has no base; give an absolute path.")?
            .join(path)
    };
    let pages = input
        .get("pages")
        .and_then(Value::as_str)
        .map(parse_page_spec)
        .transpose()
        .map_err(|error| error.to_string())?;
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(str::to_string);
    Ok(PdfReadRequest {
        path,
        pages,
        query,
        budget_chars: READ_BUDGET_CHARS,
    })
}

pub const MAX_ATTACHMENTS: usize = 5;

/// Add dropped or picked files to a draft's attachments, keeping only PDFs,
/// skipping repeats, and stopping at [`MAX_ATTACHMENTS`].
pub fn attach(attachments: &mut Vec<PathBuf>, paths: impl IntoIterator<Item = PathBuf>) {
    for path in paths {
        let is_pdf = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
        if is_pdf && !attachments.contains(&path) && attachments.len() < MAX_ATTACHMENTS {
            attachments.push(path);
        }
    }
}

/// The message as the model receives it: the user's text, then each attachment
/// named by path so the model can `read_pdf` it. Backticks keep a path with
/// spaces in one piece.
pub fn message_with_attachments(text: &str, attachments: &[PathBuf]) -> String {
    let mut message = text.trim().to_string();
    for path in attachments {
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str(&format!("Attached PDF: `{}`", path.display()));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_plain_relative_path_is_known_to_stay_in_the_project() {
        for inside in ["paper.pdf", "refs/si/paper.pdf", "./paper.pdf"] {
            assert!(
                !reads_outside_project(&json!({ "path": inside })),
                "{inside}"
            );
        }
        for outside in ["/etc/paper.pdf", "../paper.pdf", "refs/../../paper.pdf"] {
            assert!(
                reads_outside_project(&json!({ "path": outside })),
                "{outside}"
            );
        }
    }

    // Backslashes and drive letters are ordinary file-name characters elsewhere.
    #[cfg(windows)]
    #[test]
    fn windows_rooted_and_drive_paths_leave_the_project() {
        for outside in ["\\Windows\\paper.pdf", "C:\\paper.pdf", "C:paper.pdf"] {
            assert!(
                reads_outside_project(&json!({ "path": outside })),
                "{outside}"
            );
        }
        assert!(!reads_outside_project(
            &json!({ "path": "refs\\paper.pdf" })
        ));
    }

    #[test]
    fn relative_paths_resolve_against_the_project_root() {
        let root = Path::new("/projects/demo");
        let request = parse_request(
            &json!({ "path": "refs/paper.pdf", "pages": "2-4", "query": "  " }),
            Some(root),
        )
        .unwrap();
        assert_eq!(request.path, root.join("refs/paper.pdf"));
        assert_eq!(request.pages, Some(2..=4));
        assert_eq!(request.query, None, "a blank query means read, not search");
    }

    #[test]
    fn a_relative_path_without_a_project_is_rejected() {
        assert!(parse_request(&json!({ "path": "paper.pdf" }), None).is_err());
        assert!(parse_request(&json!({ "path": "/abs/paper.pdf" }), None).is_ok());
    }

    #[test]
    fn attaching_keeps_distinct_pdfs_up_to_the_cap() {
        let mut attachments = vec![PathBuf::from("/a/paper.pdf")];
        attach(
            &mut attachments,
            [
                PathBuf::from("/a/paper.pdf"),
                PathBuf::from("/a/structure.cif"),
                PathBuf::from("/a/SI.PDF"),
            ],
        );
        assert_eq!(
            attachments,
            [PathBuf::from("/a/paper.pdf"), PathBuf::from("/a/SI.PDF")]
        );
        attach(
            &mut attachments,
            (0..10).map(|i| PathBuf::from(format!("/a/{i}.pdf"))),
        );
        assert_eq!(attachments.len(), MAX_ATTACHMENTS);
    }

    #[test]
    fn the_sent_message_names_each_attachment_by_path() {
        let attachments = [PathBuf::from("/a/my paper.pdf")];
        assert_eq!(
            message_with_attachments(" summarize ", &attachments),
            "summarize\nAttached PDF: `/a/my paper.pdf`"
        );
        assert_eq!(
            message_with_attachments("", &attachments),
            "Attached PDF: `/a/my paper.pdf`"
        );
        assert_eq!(message_with_attachments("hi", &[]), "hi");
    }
}
