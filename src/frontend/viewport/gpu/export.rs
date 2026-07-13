//! Offscreen PNG export: render the molecule scene into a standalone texture
//! (no egui surface) and read it back to disk. Shares the same
//! [`MoleculeRenderer`] and WGSL pipelines as the live viewport.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use eframe::{egui, wgpu};

use super::super::camera::Projector;
use super::{DEPTH_FORMAT, GpuExporter, MoleculeInstances, MoleculeRenderer, camera_uniform};

pub(in crate::frontend::viewport) fn export_png(
    exporter: &GpuExporter,
    instances: &MoleculeInstances,
    projector: &Projector,
    width: u32,
    height: u32,
    background: egui::Color32,
    output_path: &Path,
) -> Result<()> {
    if width == 0 || height == 0 {
        bail!("image dimensions must be non-zero");
    }
    let max_dimension = exporter.device.limits().max_texture_dimension_2d;
    if width > max_dimension || height > max_dimension {
        bail!(
            "image size {width}x{height} exceeds the GPU limit of {max_dimension} pixels per side"
        );
    }

    let device = &exporter.device;
    let queue = &exporter.queue;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = MoleculeRenderer::new(device, format);
    renderer.write_camera(queue, camera_uniform(projector));
    renderer.upload(device, queue, instances);

    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("molecule_export_color"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("molecule_export_depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("molecule_export_encoder"),
    });
    let clear = background.to_normalized_gamma_f32();
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("molecule_export_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64,
                        g: clear[1] as f64,
                        b: clear[2] as f64,
                        a: clear[3] as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.draw(&mut pass);
    }

    let unpadded_bytes_per_row = width * 4;
    let bytes_per_row = unpadded_bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("molecule_export_readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("failed to wait for GPU image readback")?;
    receiver
        .recv()
        .context("GPU image readback callback was dropped")?
        .context("failed to map GPU image readback buffer")?;
    let data = slice.get_mapped_range();
    let mut pixels =
        Vec::with_capacity((u64::from(unpadded_bytes_per_row) * u64::from(height)) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        pixels.extend_from_slice(&data[start..start + unpadded_bytes_per_row as usize]);
    }
    drop(data);
    readback.unmap();
    let image = image::RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| anyhow!("GPU image readback returned an invalid buffer size"))?;
    image
        .save(output_path)
        .with_context(|| format!("failed to save {}", output_path.display()))
}
