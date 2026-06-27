use super::*;
use serde_json::json;
use std::time::Duration;

#[test]
fn apply_metrics_sampler_starts_and_stops() {
    let mut jobs = JobManager::default();
    let interval = Some(Duration::from_millis(500));
    apply_metrics_sampler(&mut jobs, true, interval);
    assert!(
        jobs.metrics.is_some(),
        "turning on should spawn the sampler"
    );
    apply_metrics_sampler(&mut jobs, true, interval); // idempotent — no second sampler
    assert!(jobs.metrics.is_some());
    apply_metrics_sampler(&mut jobs, false, None);
    assert!(
        jobs.metrics.is_none(),
        "turning off should drop the sampler"
    );
}

#[test]
fn refresh_interval_maps_rates_and_pauses() {
    use crate::backend::config::MonitorRefresh;
    assert_eq!(
        refresh_interval(MonitorRefresh::High),
        Some(Duration::from_millis(500))
    );
    assert_eq!(
        refresh_interval(MonitorRefresh::Standard),
        Some(Duration::from_millis(1000))
    );
    assert_eq!(
        refresh_interval(MonitorRefresh::Low),
        Some(Duration::from_secs(4))
    );
    assert_eq!(refresh_interval(MonitorRefresh::Pause), None);
}

#[test]
fn gpu_interval_floors_and_backs_off_when_idle() {
    use crate::frontend::gpu_monitor::GpuSample;
    let sample = |util: Option<f32>| GpuSample {
        pci_bus_id: "01:00.0".into(),
        util_pct: util,
        vram_used_bytes: None,
        vram_total_bytes: None,
        temp_c: None,
    };
    // No readings yet: hold the floor so cards are still discovered promptly.
    assert_eq!(
        gpu_interval(Duration::from_millis(500), &[]),
        GPU_MIN_INTERVAL
    );
    // A busy card: floored to the minimum even at the fastest base rate.
    assert_eq!(
        gpu_interval(Duration::from_millis(500), &[sample(Some(73.0))]),
        GPU_MIN_INTERVAL
    );
    // An idle card: stretched to the longer back-off interval.
    assert_eq!(
        gpu_interval(Duration::from_millis(500), &[sample(Some(0.0))]),
        GPU_IDLE_INTERVAL
    );
    // A slow base rate still wins when it exceeds the floor.
    assert_eq!(
        gpu_interval(Duration::from_secs(30), &[sample(Some(90.0))]),
        Duration::from_secs(30)
    );
}

#[test]
fn parse_model_ids_reads_data_id_list() {
    let json = json!({ "data": [{ "id": "x" }, { "id": "y" }] });
    assert_eq!(parse_model_ids(&json), vec!["x", "y"]);
}

#[test]
fn parse_model_ids_ignores_garbage() {
    // Wrong shape, missing `data`, or non-object items all yield nothing.
    assert!(parse_model_ids(&json!({ "models": ["x"] })).is_empty());
    assert!(parse_model_ids(&json!([1, 2, 3])).is_empty());
    assert!(parse_model_ids(&json!("nope")).is_empty());
    // Items without a string `id` are skipped, not faked.
    assert_eq!(
        parse_model_ids(&json!({ "data": [{ "id": "ok" }, { "name": "no-id" }] })),
        vec!["ok"]
    );
}

#[test]
fn interpret_models_response_reads_ids_on_ok() {
    assert_eq!(
        interpret_models_response(200, r#"{"data":[{"id":"x"},{"id":"y"}]}"#),
        Ok(vec!["x".to_string(), "y".to_string()])
    );
}

#[test]
fn interpret_models_response_html_points_at_base_url() {
    // The exact symptom the user hit: Base URL without `/v1` returns the
    // relay's web page, not JSON. The error must read like the assistant path —
    // name the HTML page and point at the `/v1` API root, not raw serde.
    let err = interpret_models_response(200, "<!doctype html><html></html>").unwrap_err();
    assert!(err.contains("HTML"), "got: {err}");
    assert!(err.contains("/v1"), "got: {err}");
    assert!(!err.contains("malformed"), "leaks serde wording: {err}");
}

#[test]
fn interpret_models_response_empty_body_flags_base_url() {
    let err = interpret_models_response(200, "   ").unwrap_err();
    assert!(err.contains("empty"), "got: {err}");
}

#[test]
fn interpret_models_response_non_json_error_page_hints_url_regardless_of_status() {
    // A wrong Base URL can 404 to an HTML page too; that is still a
    // wrong-URL signal, so it gets the same hint rather than a bare status.
    let err = interpret_models_response(404, "<html>not found</html>").unwrap_err();
    assert!(err.contains("HTML"), "got: {err}");
}

#[test]
fn interpret_models_response_json_error_reports_status() {
    // A valid JSON body with a non-200 status is a real API error, not a
    // wrong URL — surface the status.
    let err = interpret_models_response(503, r#"{"error":"nope"}"#).unwrap_err();
    assert!(err.contains("503"), "got: {err}");
}

#[test]
fn interpret_models_response_json_error_reports_message() {
    let err = interpret_models_response(
        401,
        r#"{"code":"API_KEY_REQUIRED","message":"API key is required"}"#,
    )
    .unwrap_err();
    assert!(err.contains("401"), "got: {err}");
    assert!(err.contains("API key is required"), "got: {err}");
}
