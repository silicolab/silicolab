use super::*;

fn constraint(id: &str, text: &str) -> Record {
    Record {
        id: id.into(),
        category: Category::Memory,
        storage_version: 1,
        content_version: 1,
        revision: 1,
        source: Source::UserApproval {
            session: 7,
            call: "user-click".into(),
        },
        scope: Scope {
            task: Some(3),
            session: Some(7),
            run_uuid: None,
        },
        created_at_ms: 1,
        supersedes: None,
        invalidated: None,
        brief: text.into(),
        input_entries: vec![],
        result_entries: vec![],
        artifacts: vec![],
        content: Content::Constraint { text: text.into() },
    }
}
#[test]
fn explicit_replacement_preserves_history_and_scope() {
    let mut store = RecordStore::default();
    store.insert(constraint("first", "Use water")).unwrap();
    store.insert(constraint("conflict", "Use ethanol")).unwrap();
    assert!(store.active(store.get("first").unwrap()));
    let mut next = constraint("next", "Use methanol");
    next.supersedes = Some("first".into());
    store.insert(next).unwrap();
    assert!(!store.active(store.get("first").unwrap()));
    assert!(store.active(store.get("conflict").unwrap()));
    assert!(!store.context(Some(3), None, 7, 1000).contains("Use water"));
    assert!(store.context(Some(4), None, 7, 1000).is_empty());
    assert!(store.context(Some(3), None, 8, 1000).is_empty());
    assert!(store.context(Some(3), None, 7, 100).contains("omitted"));
}
#[test]
fn inferred_constraint_is_rejected_and_unknown_rows_are_preserved() {
    let mut r = constraint("c", "Use water");
    r.source = Source::Model {
        generator: "agent".into(),
        model: "m".into(),
        bases: vec![],
    };
    assert!(RecordStore::default().insert(r).is_err());
    let mut r = constraint("v2", "future");
    r.content_version = 2;
    let json = serde_json::to_string(&r).unwrap();
    let mut store = RecordStore::default();
    store.restore(r.id, json.clone());
    store.restore("broken".into(), "{".into());
    assert_eq!(store.unavailable["v2"].0, json);
    assert!(
        store
            .require("broken")
            .unwrap_err()
            .to_string()
            .contains("unavailable")
    );
}
#[test]
fn non_qm_adapter_roundtrips_and_duplicate_content_cannot_replace_fact() {
    let mut r = constraint("test", "lightweight adapter");
    r.category = Category::Evidence;
    r.source = Source::Program { job: "job".into() };
    r.content = Content::TestMeasurement { value: 42 };
    let mut store = RecordStore::default();
    assert!(store.insert(r.clone()).unwrap());
    assert!(!store.insert(r.clone()).unwrap());
    let mut restored = RecordStore::default();
    restored.restore(r.id.clone(), serde_json::to_string(&r).unwrap());
    assert_eq!(
        restored.get("test").unwrap().content.kind(),
        "test.measurement"
    );
    r.content = Content::TestMeasurement { value: 43 };
    assert!(store.insert(r).is_err());
}
#[test]
fn derived_basis_invalidation_is_visible() {
    let mut store = RecordStore::default();
    store.insert(constraint("c", "Use water")).unwrap();
    let mut r = constraint("d", "interpretation");
    r.category = Category::Derived;
    r.content = Content::Explanation {
        text: "interpretation".into(),
    };
    r.source = Source::Model {
        generator: "agent".into(),
        model: "m".into(),
        bases: vec![Basis {
            id: "c".into(),
            revision: 1,
        }],
    };
    store.insert(r).unwrap();
    let mut next = constraint("next", "Use ethanol");
    next.supersedes = Some("c".into());
    store.insert(next).unwrap();
    assert!(store.stale(store.get("d").unwrap()));
}

fn explanation(id: &str, bases: &[&str]) -> Record {
    let mut record = constraint(id, "interpretation");
    record.category = Category::Derived;
    record.content = Content::Explanation {
        text: "interpretation".into(),
    };
    record.source = Source::Model {
        generator: "agent".into(),
        model: "m".into(),
        bases: bases
            .iter()
            .map(|id| Basis {
                id: (*id).into(),
                revision: 1,
            })
            .collect(),
    };
    record
}

#[test]
fn derived_staleness_propagates_and_stale_bases_cannot_be_reused() {
    let mut store = RecordStore::default();
    store.insert(constraint("c", "Use water")).unwrap();
    store.insert(explanation("d1", &["c"])).unwrap();
    store.insert(explanation("d2", &["d1"])).unwrap();
    store.insert(explanation("diamond", &["d1", "d2"])).unwrap();
    assert!(!store.stale(store.get("diamond").unwrap()));
    let mut replacement = constraint("next", "Use ethanol");
    replacement.supersedes = Some("c".into());
    store.insert(replacement).unwrap();
    for id in ["d1", "d2", "diamond"] {
        assert!(store.stale(store.get(id).unwrap()));
    }
    assert!(store.insert(explanation("new", &["d2"])).is_err());
}

#[test]
fn restored_dependency_cycles_and_missing_or_invalid_bases_are_stale() {
    for bad_base in ["cycle", "missing", "invalidated", "revision", "unavailable"] {
        let mut store = RecordStore::default();
        let root = explanation("root", &["child"]);
        let child = explanation("child", &["base"]);
        for record in [root, child] {
            store.restore(record.id.clone(), serde_json::to_string(&record).unwrap());
        }
        let mut base = constraint("base", "Use water");
        match bad_base {
            "cycle" => base = explanation("base", &["root"]),
            "missing" => (),
            "invalidated" => base.invalidated = Some("withdrawn".into()),
            "revision" => base.revision = 2,
            "unavailable" => base.content_version = 2,
            _ => unreachable!(),
        }
        if bad_base != "missing" {
            store.restore(base.id.clone(), serde_json::to_string(&base).unwrap());
        }
        assert!(store.stale(store.get("root").unwrap()), "{bad_base}");
        assert!(store.insert(explanation("new", &["root"])).is_err());
    }
    assert!(
        RecordStore::default()
            .insert(explanation("self", &["self"]))
            .is_err()
    );
}

#[test]
fn deep_restored_dependencies_do_not_require_recursion() {
    let mut store = RecordStore::default();
    store.insert(constraint("0", "Use water")).unwrap();
    for n in 1..=2048 {
        let record = explanation(&n.to_string(), &[&(n - 1).to_string()]);
        store.restore(record.id.clone(), serde_json::to_string(&record).unwrap());
    }
    assert!(!store.stale(store.get("2048").unwrap()));
    let mut replacement = constraint("replacement", "Use ethanol");
    replacement.supersedes = Some("0".into());
    store.insert(replacement).unwrap();
    assert!(store.stale(store.get("2048").unwrap()));
}
#[test]
fn report_pages_are_bounded_utf8_and_paths_cannot_escape() {
    let root = std::env::temp_dir().join(format!("silicolab-records-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let text = "证据\n".repeat(1000);
    std::fs::write(root.join("report.txt"), &text).unwrap();
    let mut a = artifacts::Artifact {
        name: "report".into(),
        task: 1,
        relative: "report.txt".into(),
    };
    let mut offset = 0;
    let mut restored = String::new();
    loop {
        let page = artifacts::read_page(&root, &a, offset, 17).unwrap();
        assert!(page.text.len() <= 17);
        restored.push_str(&page.text);
        offset = page.next_offset;
        if !page.truncated {
            break;
        }
    }
    assert_eq!(restored, text);
    assert!(artifacts::read_page(&root, &a, 1, 17).is_err());
    a.relative = "../report.txt".into();
    assert!(artifacts::read_page(&root, &a, 0, 17).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn qm_facts_do_not_invent_empty_or_nonfinite_values() {
    let o = crate::engines::qm::QmOutcome {
        energy_hartree: f64::NAN,
        converged: false,
        optimized_structure: None,
        summary: "warning".repeat(10000),
        scf_trace: vec![],
        opt_trace: vec![],
        frequencies: vec![f64::NAN, -3.0, 20.0],
    };
    let facts = qm::QmFacts::from_outcome(&o, None);
    assert_eq!(facts.energy_hartree, None);
    assert_eq!(facts.frequency_range_cm_inverse, Some([-3.0, 20.0]));
    assert_eq!(facts.frequencies.non_finite, 1);
    let series = crate::backend::runs::QmSeries::from_outcome(&o);
    let error = crate::backend::runs::save_qm_series_file(std::path::Path::new("unused"), &series)
        .unwrap_err();
    assert!(error.to_string().contains("non-finite"));
    assert!(facts.scf.availability.contains("not proof"));
    assert!(qm::completion("job", &o).len() < 400);
}

#[test]
fn reused_task_number_does_not_restore_another_runs_constraint() {
    let mut record = constraint("scoped", "Use water");
    record.scope.run_uuid = Some("original-run".into());
    let mut store = RecordStore::default();
    store.insert(record).unwrap();
    assert!(
        store
            .context(Some(3), Some("original-run"), 7, 1000)
            .contains("Use water")
    );
    assert!(
        store
            .context(Some(3), Some("another-run"), 7, 1000)
            .is_empty()
    );
}
