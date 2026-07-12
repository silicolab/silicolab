use super::retention::{JOB_SCOPE_MAX_BYTES, TOTAL_MAX_BYTES};
use super::*;
use crate::job::JobId;

fn command(id: CommandId, level: LogLevel, text: &str) -> NewLogEntry {
    NewLogEntry::new(
        LogScope::Command {
            command_id: id,
            actor: CommandActor::User,
        },
        level,
        text,
    )
}

fn system(text: &str) -> NewLogEntry {
    NewLogEntry::new(
        LogScope::System {
            subsystem: SystemSubsystem::Storage,
        },
        LogLevel::Info,
        text,
    )
}

fn job(job_id: JobId, text: &str) -> NewLogEntry {
    NewLogEntry::new(LogScope::Job { job_id }, LogLevel::Info, text)
}

#[test]
fn sequence_is_monotonic_including_folds() {
    let mut store = SessionLogStore::default();
    store.append(system("a"));
    store.append(system("a")); // fold
    store.append(system("b"));
    let seqs: Vec<_> = store
        .query(&LogQuery::new(LogFilter::OutputAll))
        .map(|e| (e.first_seq, e.last_seq))
        .collect();
    // Two rows: the folded "a" (first_seq 0, last_seq 1) and "b" (first_seq 2).
    assert_eq!(seqs, vec![(0, 1), (2, 2)]);
    assert_eq!(store.next_seq(), 3);
}

#[test]
fn folding_requires_global_adjacency() {
    let mut store = SessionLogStore::default();
    let a = JobId::new();
    let b = JobId::new();
    store.append(job(a, "line"));
    store.append(job(b, "line")); // different scope between
    store.append(job(a, "line")); // not adjacent to the first -> new row
    let rows: Vec<_> = store
        .tail_for_job(a, 10)
        .map(|e| e.repeat_count())
        .collect();
    assert_eq!(rows, vec![1, 1]);
}

#[test]
fn folding_requires_matching_level_and_text() {
    let mut store = SessionLogStore::default();
    store.append(command(1, LogLevel::Info, "x"));
    store.append(command(1, LogLevel::Error, "x")); // level differs
    store.append(command(1, LogLevel::Error, "x")); // folds with previous
    store.append(command(1, LogLevel::Error, "y")); // text differs
    let rows: Vec<_> = store
        .query(&LogQuery::new(LogFilter::Command))
        .map(|e| (e.level, e.repeat_count()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (LogLevel::Info, 1),
            (LogLevel::Error, 2),
            (LogLevel::Error, 1),
        ]
    );
}

#[test]
fn folding_preserves_first_seq_and_position() {
    let mut store = SessionLogStore::default();
    store.append(system("keep"));
    for _ in 0..5 {
        store.append(system("keep"));
    }
    let query = LogQuery::new(LogFilter::OutputAll);
    let entry = store.query(&query).next().unwrap();
    assert_eq!(entry.first_seq, 0);
    assert_eq!(entry.repeat_count(), 6);
    assert_eq!(entry.last_seq, 5);
}

#[test]
fn crlf_normalizes_without_touching_other_text() {
    let mut store = SessionLogStore::default();
    store.append(system("a\r\nb"));
    store.append(system("a\nb")); // identical after normalization -> fold
    let query = LogQuery::new(LogFilter::OutputAll);
    let entry = store.query(&query).next().unwrap();
    assert_eq!(entry.text, "a\nb");
    assert_eq!(entry.repeat_count(), 2);
}

#[test]
fn oversized_entry_truncates_on_char_boundary_with_marker() {
    let mut store = SessionLogStore::default();
    // Multi-byte chars straddling the limit must not be split.
    let text = "é".repeat(200_000);
    store.append(system(&text));
    let query = LogQuery::new(LogFilter::OutputAll);
    let entry = store.query(&query).next().unwrap();
    assert!(entry.text.is_char_boundary(entry.text.len()));
    assert!(entry.text.len() <= 64 * 1024);
    assert!(entry.text.contains("bytes truncated"));
    // A truncated entry is a valid str (would panic on an invalid boundary).
    assert!(entry.text.chars().count() > 0);
}

#[test]
fn per_job_cap_bounds_one_scope() {
    let mut store = SessionLogStore::default();
    let noisy = JobId::new();
    for i in 0..400 {
        // Distinct text so lines never fold: 400 * ~4 KiB into a 1 MiB per-job cap.
        store.append(job(noisy, &format!("line {i} {}", "x".repeat(4096))));
    }
    let job_bytes: usize = store
        .tail_for_job(noisy, usize::MAX)
        .map(|e| e.text.len())
        .sum();
    assert!(job_bytes <= JOB_SCOPE_MAX_BYTES);
    // The thinned job scope carries exactly one eviction marker.
    let markers = store
        .tail_for_job(noisy, usize::MAX)
        .filter(|e| e.text.contains("discarded"))
        .count();
    assert_eq!(markers, 1);
}

#[test]
fn noisy_job_cannot_evict_the_command_reserve() {
    let mut store = SessionLogStore::default();
    // Seed command + system diagnostics well within their reserves.
    for i in 0..20 {
        store.append(command(i, LogLevel::Error, &format!("command error {i}")));
        store.append(system(&format!("system detail {i}")));
    }
    let command_before = store.query(&LogQuery::new(LogFilter::Command)).count();
    // Flood distinct jobs past the total budget.
    let line = "y".repeat(8192);
    for _ in 0..2400 {
        let id = JobId::new();
        store.append(job(id, &line));
    }
    assert!(store.retained_bytes() <= TOTAL_MAX_BYTES);
    let command_after = store.query(&LogQuery::new(LogFilter::Command)).count();
    assert_eq!(
        command_before, command_after,
        "command diagnostics within reserve must survive a job flood"
    );
}

#[test]
fn eviction_marker_does_not_recurse() {
    let mut store = SessionLogStore::default();
    let noisy = JobId::new();
    for i in 0..600 {
        store.append(job(noisy, &format!("{i} {}", "z".repeat(8192))));
    }
    // Repeated eviction from one scope keeps exactly one marker (folded), never a
    // growing pile of warnings.
    let markers = store
        .tail_for_job(noisy, usize::MAX)
        .filter(|e| e.text.contains("discarded"))
        .count();
    assert_eq!(markers, 1);
}

#[test]
fn output_query_excludes_command() {
    let mut store = SessionLogStore::default();
    store.append(command(1, LogLevel::Info, "ran"));
    store.append(system("system"));
    let out: Vec<_> = store
        .query(&LogQuery::new(LogFilter::OutputAll))
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(out, vec!["system".to_string()]);
    let cmd: Vec<_> = store
        .query(&LogQuery::new(LogFilter::Command))
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(cmd, vec!["ran".to_string()]);
}

#[test]
fn exact_job_query_isolates_same_kind_jobs() {
    let mut store = SessionLogStore::default();
    let a = JobId::new();
    let b = JobId::new();
    store.append(job(a, "from a"));
    store.append(job(b, "from b"));
    store.append(job(a, "from a again"));
    let only_a: Vec<_> = store
        .query(&LogQuery::new(LogFilter::Job(a)))
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(
        only_a,
        vec!["from a".to_string(), "from a again".to_string()]
    );
    let tail_b: Vec<_> = store.tail_for_job(b, 10).map(|e| e.text.clone()).collect();
    assert_eq!(tail_b, vec!["from b".to_string()]);
}

#[test]
fn clear_view_cursor_hides_but_keeps_entries() {
    let mut store = SessionLogStore::default();
    store.append(system("old"));
    let cursor = store.next_seq();
    store.append(system("new"));
    let visible: Vec<_> = store
        .query(&LogQuery::new(LogFilter::OutputAll).cleared_before(cursor))
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(visible, vec!["new".to_string()]);
    // Still present without the cursor: clear-view never deletes.
    assert_eq!(store.query(&LogQuery::new(LogFilter::OutputAll)).count(), 2);
}

#[test]
fn clear_view_prevents_a_repeat_from_resurrecting_the_hidden_row() {
    let mut store = SessionLogStore::default();
    store.append(system("same"));
    let cursor = store.break_folding();
    store.append(system("same"));
    let query = LogQuery::new(LogFilter::OutputAll).cleared_before(cursor);
    let visible: Vec<_> = store.query(&query).collect();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].first_seq, cursor);
    assert_eq!(visible[0].repeat_count(), 1);
}

#[test]
fn search_is_case_insensitive_substring() {
    let mut store = SessionLogStore::default();
    store.append(system("Deploy FAILED on host"));
    store.append(system("all clear"));
    let hits: Vec<_> = store
        .query(&LogQuery::new(LogFilter::OutputAll).search("failed"))
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(hits, vec!["Deploy FAILED on host".to_string()]);
}

#[test]
fn tail_for_job_returns_last_n_oldest_first() {
    let mut store = SessionLogStore::default();
    let id = JobId::new();
    for i in 0..10 {
        store.append(job(id, &format!("line {i}")));
    }
    let tail: Vec<_> = store.tail_for_job(id, 3).map(|e| e.text.clone()).collect();
    assert_eq!(
        tail,
        vec![
            "line 7".to_string(),
            "line 8".to_string(),
            "line 9".to_string()
        ]
    );
}
