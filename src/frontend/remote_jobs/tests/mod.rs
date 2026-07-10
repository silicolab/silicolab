use super::*;

fn host_with_cores(cores: Option<usize>) -> crate::backend::config::RemoteHost {
    use crate::backend::config::{RemoteHost, ResourceSpec};
    RemoteHost {
        id: "h".into(),
        label: "H".into(),
        hostname: "example.com".into(),
        username: "alice".into(),
        resources: ResourceSpec {
            cpus_per_task: cores.map(|value| value as u32),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn requested_cores_precedence() {
    let host = host_with_cores(Some(4));
    assert_eq!(resolve_requested_cores(Some(2), &host, 16), 2); // per-job wins
    assert_eq!(resolve_requested_cores(None, &host, 16), 4); // then per-host
    let host = host_with_cores(None);
    assert_eq!(resolve_requested_cores(None, &host, 16), 16); // then fallback
}

#[test]
fn clamp_prefers_threads_then_cores_then_passthrough() {
    use crate::engines::remote::hardware::RemoteHardwareInfo;
    let both = RemoteHardwareInfo {
        threads: Some(8),
        cores: Some(4),
        ..Default::default()
    };
    assert_eq!(clamp_to_remote_inventory(32, &both), 8); // clamp to logical threads
    assert_eq!(clamp_to_remote_inventory(2, &both), 2); // already under the bound
    let phys = RemoteHardwareInfo {
        threads: None,
        cores: Some(4),
        ..Default::default()
    };
    assert_eq!(clamp_to_remote_inventory(32, &phys), 4); // fall back to physical cores
    let none = RemoteHardwareInfo::default();
    assert_eq!(clamp_to_remote_inventory(32, &none), 32); // un-probeable → pass through
    assert_eq!(clamp_to_remote_inventory(0, &none), 1); // never below 1
}

#[test]
fn remote_memory_rejection_names_host_and_advises() {
    let can_direct = MemoryVerdict::ExceedsCanDirect {
        estimate: 20_u64 << 30,
        budget: 16_u64 << 30,
    };
    let msg = remote_qm_memory_rejection(&can_direct, "cluster").expect("should reject");
    assert!(msg.contains("cluster"), "names the host: {msg}");
    assert!(
        msg.contains("integral-direct"),
        "offers the cheaper backend"
    );

    let must_reduce = MemoryVerdict::ExceedsMustReduce {
        estimate: 20_u64 << 30,
        budget: 16_u64 << 30,
    };
    let msg = remote_qm_memory_rejection(&must_reduce, "cluster").expect("should reject");
    assert!(msg.contains("cluster"));
    assert!(msg.contains("smaller"), "advises reducing the system");

    // A job that fits is not rejected.
    assert!(remote_qm_memory_rejection(&MemoryVerdict::Ok, "cluster").is_none());
}

/// End-to-end tests against a real SSH host. Split out because they need an
/// environment (an SSH host with a deployed worker), not just a process.
#[cfg(feature = "dev-worker")]
mod e2e;
