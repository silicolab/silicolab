//! One-shot wgpu enumeration of every GPU adapter on the host, mapped into the
//! backend's wgpu-free [`GpuInfo`] inventory. Run once at startup (`app.rs`) and
//! handed to [`crate::backend::hardware::set_gpu_inventory`].
//!
//! This exists because the app otherwise only knows the single *render* adapter
//! (chosen `LowPower`, so usually the integrated GPU on a dual-GPU machine). The
//! enumeration lists every adapter — discrete included — so the hardware monitor
//! can show them all. wgpu's enumeration yields names/types/ids only, never live
//! load; that is `gpu_monitor`'s job.

use eframe::wgpu;

use crate::backend::hardware::{GpuInfo, GpuKind};

/// Enumerate all GPU adapters via the platform's primary backend. One backend
/// per OS so a single physical GPU isn't reported once per backend. Returns an
/// empty vec when nothing enumerates (headless / software-only renderer), in
/// which case callers fall back to the render-adapter name.
pub(crate) fn enumerate() -> Vec<GpuInfo> {
    let backends = native_backends();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    pollster::block_on(instance.enumerate_adapters(backends))
        .iter()
        .map(|adapter| {
            let info = adapter.get_info();
            GpuInfo {
                name: info.name,
                kind: map_kind(info.device_type),
                vendor: info.vendor,
                pci_bus_id: info.device_pci_bus_id,
                backend: format!("{:?}", info.backend),
            }
        })
        .collect()
}

/// The single graphics backend to enumerate through per OS. Picking one avoids
/// the same physical GPU appearing once per backend (e.g. DX12 *and* Vulkan on
/// Windows).
fn native_backends() -> wgpu::Backends {
    #[cfg(target_os = "windows")]
    {
        wgpu::Backends::DX12
    }
    #[cfg(target_os = "macos")]
    {
        wgpu::Backends::METAL
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        wgpu::Backends::VULKAN
    }
}

fn map_kind(t: wgpu::DeviceType) -> GpuKind {
    match t {
        wgpu::DeviceType::DiscreteGpu => GpuKind::Discrete,
        wgpu::DeviceType::IntegratedGpu => GpuKind::Integrated,
        wgpu::DeviceType::VirtualGpu => GpuKind::Virtual,
        wgpu::DeviceType::Cpu => GpuKind::Cpu,
        wgpu::DeviceType::Other => GpuKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_kind_covers_every_device_type() {
        assert_eq!(map_kind(wgpu::DeviceType::DiscreteGpu), GpuKind::Discrete);
        assert_eq!(
            map_kind(wgpu::DeviceType::IntegratedGpu),
            GpuKind::Integrated
        );
        assert_eq!(map_kind(wgpu::DeviceType::VirtualGpu), GpuKind::Virtual);
        assert_eq!(map_kind(wgpu::DeviceType::Cpu), GpuKind::Cpu);
        assert_eq!(map_kind(wgpu::DeviceType::Other), GpuKind::Other);
    }

    #[test]
    fn native_backends_is_a_single_backend() {
        assert_eq!(native_backends().iter().count(), 1);
    }
}
