use super::*;

#[test]
fn a_document_block_round_trips_by_reference() {
    let block = PersistedContentBlock::Document {
        document: PersistedDocumentRef {
            path: "/papers/si.pdf".into(),
            name: "si.pdf".into(),
            bytes: 2048,
            modified_ms: 1_700_000_000_000,
            pages: 12,
        },
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "document");
    assert_eq!(json["path"], "/papers/si.pdf");
    assert_eq!(
        serde_json::from_value::<PersistedContentBlock>(json).unwrap(),
        block
    );
}

#[test]
fn a_block_type_from_a_newer_version_reads_as_unknown() {
    let block: PersistedContentBlock =
        serde_json::from_str(r#"{"type":"hologram","payload":[1,2,3]}"#).unwrap();
    assert_eq!(block, PersistedContentBlock::Unknown);
}

#[test]
fn a_user_entry_saved_before_attachments_still_loads() {
    let entry: PersistedTranscriptEntry =
        serde_json::from_str(r#"{"type":"user","text":"prepare 1ubq"}"#).unwrap();
    assert_eq!(
        entry,
        PersistedTranscriptEntry::User {
            text: "prepare 1ubq".into(),
            attachments: Vec::new(),
        }
    );
    assert_eq!(
        serde_json::to_string(&entry).unwrap(),
        r#"{"type":"user","text":"prepare 1ubq"}"#,
        "a message without attachments is written exactly as before"
    );
}
