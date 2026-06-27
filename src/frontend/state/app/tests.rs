use super::*;

#[test]
fn remote_gpu_live_apply_tracks_latest_and_history() {
    use crate::engines::remote::hardware::RemoteGpuStat;
    let mut live = RemoteGpuLive::default();
    live.apply(vec![RemoteGpuStat {
        index: 0,
        name: "GPU A".into(),
        util_pct: Some(10.0),
        vram_used_mib: Some(100),
        vram_total_mib: Some(8192),
        temp_c: Some(40),
        power_w: Some(15.0),
    }]);
    live.apply(vec![RemoteGpuStat {
        index: 0,
        name: "GPU A".into(),
        util_pct: Some(80.0),
        vram_used_mib: Some(200),
        vram_total_mib: Some(8192),
        temp_c: Some(55),
        power_w: Some(120.0),
    }]);
    assert_eq!(live.gpus.len(), 1);
    assert_eq!(live.gpus[0].latest.util_pct, Some(80.0));
    assert_eq!(live.gpus[0].util_history.len(), 2);
    assert_eq!(live.gpus[0].util_history.back().copied(), Some(Some(80.0)));
    assert!(live.last_error.is_none());
}
