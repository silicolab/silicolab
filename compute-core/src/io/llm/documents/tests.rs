use super::*;
use crate::io::llm::types::Role;
use crate::io::pdf::tests::{TempPdf, pdf_bytes};

const ROOMY: NativeLimits = NativeLimits {
    max_raw_bytes: 1024 * 1024,
    max_pages: Some(100),
};

fn live() -> AtomicBool {
    AtomicBool::new(false)
}

fn attached(file: &TempPdf, question: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: vec![
            ContentBlock::Document(describe(&file.0).unwrap()),
            ContentBlock::Text(question.into()),
        ],
    }
}

fn texts(message: &ChatMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn documents_in(message: &ChatMessage) -> usize {
    message
        .content
        .iter()
        .filter(|block| matches!(block, ContentBlock::Document(_)))
        .count()
}

#[test]
fn history_without_documents_is_borrowed_untouched() {
    let history = [ChatMessage::user_text("hi")];
    let resolved = resolve(&history, DocMode::ExtractedText, &live()).unwrap();
    assert!(matches!(resolved.messages, Cow::Borrowed(_)));
}

#[test]
fn describe_records_pages_and_size() {
    let file = TempPdf::new("describe", &pdf_bytes(&["one", "two"]));
    let document = describe(&file.0).unwrap();
    assert_eq!(document.pages, 2);
    assert_eq!(document.bytes, std::fs::metadata(&file.0).unwrap().len());
    assert!(document.name.ends_with("describe.pdf"));
}

#[test]
fn native_mode_keeps_the_block_and_encodes_the_file() {
    let file = TempPdf::new("native", &pdf_bytes(&["B3LYP"]));
    let history = [attached(&file, "which functional?")];
    let resolved = resolve(&history, DocMode::Native(ROOMY), &live()).unwrap();
    let ContentBlock::Document(document) = &resolved.messages[0].content[0] else {
        panic!("document block should stay first");
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(resolved.payload(document).unwrap())
        .unwrap();
    assert_eq!(decoded, std::fs::read(&file.0).unwrap());
    assert_eq!(texts(&resolved.messages[0]), "which functional?");
}

#[test]
fn extracted_text_mode_inlines_the_pages() {
    let file = TempPdf::new("extract", &pdf_bytes(&["def2-TZVP basis"]));
    let history = [attached(&file, "which basis?")];
    let resolved = resolve(&history, DocMode::ExtractedText, &live()).unwrap();
    assert_eq!(documents_in(&resolved.messages[0]), 0);
    let text = texts(&resolved.messages[0]);
    assert!(text.contains("def2-TZVP basis"), "{text}");
    assert!(text.contains("read_pdf"));
    assert!(text.ends_with("which basis?"));
}

#[test]
fn path_mention_mode_names_the_absolute_path() {
    let file = TempPdf::new("path", &pdf_bytes(&["x"]));
    let history = [attached(&file, "summarise")];
    let resolved = resolve(&history, DocMode::PathMention, &live()).unwrap();
    assert!(texts(&resolved.messages[0]).contains(&file.0.display().to_string()));
}

#[test]
fn over_the_page_limit_falls_back_to_text_with_the_reason() {
    let file = TempPdf::new("long", &pdf_bytes(&["p1", "p2", "p3"]));
    let history = [attached(&file, "?")];
    let limits = NativeLimits {
        max_pages: Some(2),
        ..ROOMY
    };
    let resolved = resolve(&history, DocMode::Native(limits), &live()).unwrap();
    assert_eq!(documents_in(&resolved.messages[0]), 0);
    let text = texts(&resolved.messages[0]);
    assert!(text.contains("3 pages"), "{text}");
    assert!(text.contains("2-page PDF limit"));
    assert!(text.contains("p1"));
}

#[test]
fn over_the_size_limit_falls_back_to_text() {
    let file = TempPdf::new("big", &pdf_bytes(&["payload"]));
    let history = [attached(&file, "?")];
    let limits = NativeLimits {
        max_raw_bytes: 16,
        max_pages: None,
    };
    let resolved = resolve(&history, DocMode::Native(limits), &live()).unwrap();
    assert_eq!(documents_in(&resolved.messages[0]), 0);
    assert!(texts(&resolved.messages[0]).contains("size limit"));
}

#[test]
fn a_missing_file_becomes_a_placeholder_not_an_error() {
    let file = TempPdf::new("gone", &pdf_bytes(&["x"]));
    let history = [attached(&file, "?")];
    std::fs::remove_file(&file.0).unwrap();
    for mode in [DocMode::Native(ROOMY), DocMode::ExtractedText] {
        let resolved = resolve(&history, mode, &live()).unwrap();
        assert_eq!(documents_in(&resolved.messages[0]), 0);
        assert!(texts(&resolved.messages[0]).contains("no longer available"));
    }
}

#[test]
fn a_changed_file_is_sent_current_with_a_note() {
    let file = TempPdf::new("changed", &pdf_bytes(&["old"]));
    let history = [attached(&file, "?")];
    std::fs::write(&file.0, pdf_bytes(&["new text", "second page"])).unwrap();
    let resolved = resolve(&history, DocMode::Native(ROOMY), &live()).unwrap();
    assert_eq!(documents_in(&resolved.messages[0]), 1);
    assert!(texts(&resolved.messages[0]).contains("changed on disk"));
}

#[test]
fn only_the_newest_messages_with_documents_stay_full() {
    let files: Vec<TempPdf> = (0..3)
        .map(|index| TempPdf::new("window", &pdf_bytes(&[&format!("paper {index}")])))
        .collect();
    let mut history = Vec::new();
    for file in &files {
        history.push(attached(file, "?"));
        history.push(ChatMessage::user_text("follow-up"));
    }
    let resolved = resolve(&history, DocMode::Native(ROOMY), &live()).unwrap();
    assert_eq!(documents_in(&resolved.messages[0]), 0);
    assert!(texts(&resolved.messages[0]).contains("earlier attachment"));
    assert_eq!(documents_in(&resolved.messages[2]), 1);
    assert_eq!(documents_in(&resolved.messages[4]), 1);
}

#[test]
fn cancellation_during_extraction_surfaces_as_cancelled() {
    let file = TempPdf::new("cancel", &pdf_bytes(&["x"]));
    let history = [attached(&file, "?")];
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        resolve(&history, DocMode::ExtractedText, &cancelled),
        Err(LlmError::Cancelled)
    ));
}
