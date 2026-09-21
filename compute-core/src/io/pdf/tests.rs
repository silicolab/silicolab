use super::*;

use pdf_extract::content::{Content, Operation};
use pdf_extract::{Dictionary, Object, Stream};

pub(crate) fn pdf_bytes(pages: &[&str]) -> Vec<u8> {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    let font_id = document.add_object(font);
    let mut fonts = Dictionary::new();
    fonts.set("F1", font_id);
    let mut resources = Dictionary::new();
    resources.set("Font", fonts);
    let resources_id = document.add_object(resources);

    let mut kids = Vec::new();
    for text in pages {
        let mut operations = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![72.into(), 720.into()]),
        ];
        for line in text.lines() {
            operations.push(Operation::new("Tj", vec![Object::string_literal(line)]));
            operations.push(Operation::new("Td", vec![0.into(), (-16).into()]));
        }
        operations.push(Operation::new("ET", vec![]));
        let content = Content { operations };
        let content_id =
            document.add_object(Stream::new(Dictionary::new(), content.encode().unwrap()));
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", pages_id);
        page.set("Contents", content_id);
        kids.push(Object::Reference(document.add_object(page)));
    }

    let mut tree = Dictionary::new();
    tree.set("Type", Object::Name(b"Pages".to_vec()));
    tree.set("Count", kids.len() as i64);
    tree.set("Kids", kids);
    tree.set("Resources", resources_id);
    tree.set("MediaBox", vec![0.into(), 0.into(), 595.into(), 842.into()]);
    document.objects.insert(pages_id, Object::Dictionary(tree));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", pages_id);
    let catalog_id = document.add_object(catalog);
    document.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}

pub(crate) struct TempPdf(pub(crate) PathBuf);

impl TempPdf {
    pub(crate) fn new(name: &str, bytes: &[u8]) -> Self {
        let path =
            std::env::temp_dir().join(format!("silicolab-pdf-{}-{name}.pdf", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for TempPdf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn live() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn extracts_every_page_in_order() {
    let file = TempPdf::new(
        "pages",
        &pdf_bytes(&["B3LYP functional", "def2-TZVP basis", "AMBER ff14SB"]),
    );
    let (info, pages) = extract_pages(&file.0, None, &live()).unwrap();
    assert_eq!(info.pages, 3);
    assert_eq!(
        pages.iter().map(|page| page.page).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(pages[0].text.contains("B3LYP"));
    assert!(pages[2].text.contains("ff14SB"));
}

#[test]
fn a_range_selects_only_those_pages_and_clamps_to_the_document() {
    let file = TempPdf::new("range", &pdf_bytes(&["one", "two", "three"]));
    let (_, pages) = extract_pages(&file.0, Some(2..=9), &live()).unwrap();
    assert_eq!(
        pages.iter().map(|page| page.page).collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(pages[0].text.contains("two"));
}

#[test]
fn a_start_page_past_the_end_is_an_error() {
    let file = TempPdf::new("past", &pdf_bytes(&["one"]));
    assert_eq!(
        extract_pages(&file.0, Some(4..=5), &live()),
        Err(PdfError::PageOutOfRange {
            requested: 4,
            pages: 1
        })
    );
}

#[test]
fn one_call_extracts_at_most_the_per_call_cap() {
    let texts = vec!["x"; MAX_PAGES_PER_CALL as usize + 5];
    let file = TempPdf::new("cap", &pdf_bytes(&texts));
    let (info, pages) = extract_pages(&file.0, None, &live()).unwrap();
    assert_eq!(info.pages, texts.len());
    assert_eq!(pages.len(), MAX_PAGES_PER_CALL as usize);
}

#[test]
fn page_specs_parse_single_pages_and_ranges() {
    assert_eq!(parse_page_spec("3").unwrap(), 3..=3);
    assert_eq!(parse_page_spec(" 3 - 7 ").unwrap(), 3..=7);
    for bad in ["", "0", "7-3", "a", "1-", "-2", "1-2-3"] {
        assert!(
            matches!(parse_page_spec(bad), Err(PdfError::BadPageSpec(_))),
            "{bad:?} should be rejected"
        );
    }
}

#[test]
fn search_reports_the_page_and_context_case_insensitively() {
    let file = TempPdf::new(
        "search",
        &pdf_bytes(&["Introduction", "We used the PBE0 functional throughout"]),
    );
    let (_, hits) = search(&file.0, "pbe0", &live()).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].page, 2);
    assert!(hits[0].snippet.contains("PBE0 functional"));
    assert!(search(&file.0, "CCSD", &live()).unwrap().1.is_empty());
    assert!(search(&file.0, "  ", &live()).unwrap().1.is_empty());
}

#[test]
fn rendering_stops_at_a_page_boundary_and_names_the_next_selection() {
    let info = PdfInfo { pages: 3, bytes: 0 };
    let pages: Vec<PageText> = (1..=3)
        .map(|page| PageText {
            page,
            text: "word ".repeat(60),
        })
        .collect();
    let rendered = render_paged(&info, &pages, 900);
    assert!(rendered.chars().count() <= 900);
    assert!(rendered.contains("[page 2]"));
    assert!(!rendered.contains("[page 3]"));
    assert!(rendered.contains("pages=3-3"));

    let all = render_paged(&info, &pages, 10_000);
    assert!(all.contains("[page 3]"));
    assert!(!all.contains("Continue with"));
}

#[test]
fn an_oversized_single_page_is_cut_and_says_so() {
    let info = PdfInfo { pages: 1, bytes: 0 };
    let pages = [PageText {
        page: 1,
        text: "x".repeat(5_000),
    }];
    let rendered = render_paged(&info, &pages, 1_000);
    assert!(rendered.chars().count() <= 1_000);
    assert!(rendered.contains("was cut"));
}

#[test]
fn pages_without_a_text_layer_are_reported() {
    let file = TempPdf::new("blank", &pdf_bytes(&["", ""]));
    let (info, pages) = extract_pages(&file.0, None, &live()).unwrap();
    assert!(render_paged(&info, &pages, 4_000).contains("No text layer"));
}

#[test]
fn hostile_inputs_are_errors_not_panics() {
    let good = pdf_bytes(&["text"]);
    let mut garbage = b"%PDF-1.5\n".to_vec();
    garbage.extend((0..4096u32).map(|i| (i * 31 % 251) as u8));
    let cases: [(&str, Vec<u8>); 4] = [
        ("empty", Vec::new()),
        ("text", b"not a pdf at all".to_vec()),
        ("truncated", good[..good.len() / 3].to_vec()),
        ("garbage", garbage),
    ];
    for (name, bytes) in cases {
        let file = TempPdf::new(name, &bytes);
        assert!(probe(&file.0).is_err(), "{name} should not probe");
        assert!(
            extract_pages(&file.0, None, &live()).is_err(),
            "{name} should not extract"
        );
    }
}

#[test]
fn a_missing_file_and_a_directory_are_distinguished() {
    let missing = std::env::temp_dir().join("silicolab-pdf-definitely-missing.pdf");
    assert_eq!(probe(&missing), Err(PdfError::NotFound(missing.clone())));
    assert_eq!(probe(&std::env::temp_dir()), Err(PdfError::NotAPdf));
}

#[test]
fn size_and_page_limits_are_enforced() {
    let file = TempPdf::new("limits", &pdf_bytes(&["a", "b", "c"]));
    let small = Limits {
        max_bytes: 10,
        max_pages: 100,
    };
    assert!(matches!(
        extract_pages_with(&file.0, None, &live(), small),
        Err(PdfError::TooLarge { limit: 10, .. })
    ));
    let few = Limits {
        max_bytes: MAX_PDF_BYTES,
        max_pages: 2,
    };
    assert_eq!(
        extract_pages_with(&file.0, None, &live(), few),
        Err(PdfError::TooManyPages { pages: 3, limit: 2 })
    );
}

#[test]
fn a_set_cancel_flag_stops_extraction() {
    let file = TempPdf::new("cancel", &pdf_bytes(&["a", "b"]));
    let cancel = AtomicBool::new(true);
    assert_eq!(
        extract_pages(&file.0, None, &cancel),
        Err(PdfError::Cancelled)
    );
    assert_eq!(search(&file.0, "a", &cancel), Err(PdfError::Cancelled));
}
