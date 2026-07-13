//! GPU-accelerated molecular rendering.
//!
//! Atoms are drawn as instanced billboard sphere impostors and bonds as
//! instanced capsule meshes, all in a single egui paint callback that draws
//! into egui's render pass against its depth buffer (enabled via
//! `NativeOptions::depth_buffer` in `app.rs`). The shaders (`molecule.wgsl`)
//! replicate the CPU [`Projector`] projection and the CPU "paper" shading so the
//! GPU view lines up with the still-CPU-drawn cell box / labels / picking and
//! looks the same as before — only far faster, because per-atom data is uploaded
//! once and rotation just updates a small camera uniform.

use bytemuck::{Pod, Zeroable};
use eframe::{egui, egui_wgpu, wgpu};
use nalgebra::Vector3;

use super::camera::Projector;

/// Depth format of egui's render pass. Must match `NativeOptions::depth_buffer`
/// (32 bits → [`wgpu::TextureFormat::Depth32Float`]) configured in `app.rs`.
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const CYLINDER_SIDES: usize = 16;
const CAPSULE_CAP_RINGS: usize = 5;

// ---------------------------------------------------------------------------
// GPU data (std430/vertex-buffer layouts; kept in sync with `molecule.wgsl`).

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct SphereInstance {
    /// xyz = world center, w = world radius.
    pub(super) pos_radius: [f32; 4],
    /// rgba in gamma [0,1].
    pub(super) color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct CylinderInstance {
    /// xyz = world start, w = length.
    pub(super) start_len: [f32; 4],
    /// xyz = unit axis (start→end), w = radius.
    pub(super) axis_radius: [f32; 4],
    /// xyz = U basis (perpendicular to axis).
    pub(super) side_u: [f32; 4],
    /// xyz = V basis (perpendicular to axis and U).
    pub(super) side_v: [f32; 4],
    /// start-half color, rgba gamma.
    pub(super) color_a: [f32; 4],
    /// end-half color, rgba gamma.
    pub(super) color_b: [f32; 4],
}

/// A world-space triangle-mesh vertex (cartoon ribbons, molecular surface),
/// drawn through the general mesh pipeline. Triangles are stored as a flat soup
/// (every three vertices form a triangle); the surface mesh carries a sub-1.0
/// alpha for transparency.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct MeshVertex {
    pub(super) position: [f32; 3],
    pub(super) normal: [f32; 3],
    pub(super) color: [f32; 4],
}

/// Camera-independent per-frame geometry. Rebuilt only when the structure,
/// styling, or selection changes — never on camera movement. Holds the
/// instanced primitives (atoms as spheres, bonds as cylinders) and the meshed
/// representations (cartoon ribbons, opaque; molecular surface, transparent).
#[derive(Default)]
pub(super) struct MoleculeInstances {
    pub(super) spheres: Vec<SphereInstance>,
    pub(super) cylinders: Vec<CylinderInstance>,
    pub(super) cartoon: Vec<MeshVertex>,
    pub(super) surface: Vec<MeshVertex>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    /// Columns rotate world→view (yaw then pitch). Upper-left 3x3; rest identity.
    rotation: [[f32; 4]; 4],
    center: [f32; 4],
    /// camera_distance, near_plane, scale, srgb_framebuffer flag.
    params: [f32; 4],
    /// local_center.x, local_center.y, rect_width, rect_height.
    screen: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CylinderVertex {
    /// Radial xy, axial length fraction, and radius-scaled axial offset/normal.
    local: [f32; 4],
}

/// Build the camera uniform from a [`Projector`], mirroring `Projector::project`
/// exactly. The columns of the rotation matrix are `rotate(e_x/e_y/e_z)`, so
/// `rotation * v == Projector::view_space(v + center)`. The `srgb` flag is filled
/// in later (in `prepare`) from the framebuffer format.
fn camera_uniform(projector: &Projector) -> CameraUniform {
    let col0 = projector.rotate_to_view(Vector3::new(1.0, 0.0, 0.0));
    let col1 = projector.rotate_to_view(Vector3::new(0.0, 1.0, 0.0));
    let col2 = projector.rotate_to_view(Vector3::new(0.0, 0.0, 1.0));
    let near_plane = (projector.camera_distance * 0.2).max(0.1);
    let rect = projector.rect;
    let local_center_x = rect.width() * 0.5 + projector.pan.x;
    let local_center_y = rect.height() * 0.5 + projector.pan.y;
    CameraUniform {
        rotation: [
            [col0.x, col0.y, col0.z, 0.0],
            [col1.x, col1.y, col1.z, 0.0],
            [col2.x, col2.y, col2.z, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        center: [
            projector.center.x,
            projector.center.y,
            projector.center.z,
            0.0,
        ],
        params: [projector.camera_distance, near_plane, projector.scale, 0.0],
        screen: [local_center_x, local_center_y, rect.width(), rect.height()],
    }
}

fn capsule_ring_vertex(
    side: usize,
    latitude: f32,
    axial_fraction: f32,
    axial_sign: f32,
) -> [f32; 4] {
    use std::f32::consts::TAU;
    let azimuth = TAU * side as f32 / CYLINDER_SIDES as f32;
    let (azimuth_sin, azimuth_cos) = azimuth.sin_cos();
    let (axial, radial) = latitude.sin_cos();
    [
        radial * azimuth_cos,
        radial * azimuth_sin,
        axial_fraction,
        axial_sign * axial,
    ]
}

fn unit_capsule_vertices() -> Vec<CylinderVertex> {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let cap_vertices_per_side = (CAPSULE_CAP_RINGS - 1) * 6 + 3;
    let mut vertices = Vec::with_capacity(CYLINDER_SIDES * (6 + cap_vertices_per_side * 2));

    for side in 0..CYLINDER_SIDES {
        let a0 = TAU * side as f32 / CYLINDER_SIDES as f32;
        let a1 = TAU * (side + 1) as f32 / CYLINDER_SIDES as f32;
        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();
        let v00 = [c0, s0, 0.0, 0.0];
        let v10 = [c1, s1, 0.0, 0.0];
        let v11 = [c1, s1, 1.0, 0.0];
        let v01 = [c0, s0, 1.0, 0.0];
        for local in [v00, v10, v11, v00, v11, v01] {
            vertices.push(CylinderVertex { local });
        }
    }

    for (axial_fraction, axial_sign) in [(0.0, -1.0), (1.0, 1.0)] {
        for ring in 0..CAPSULE_CAP_RINGS {
            let latitude0 = FRAC_PI_2 * ring as f32 / CAPSULE_CAP_RINGS as f32;
            let latitude1 = FRAC_PI_2 * (ring + 1) as f32 / CAPSULE_CAP_RINGS as f32;
            for side in 0..CYLINDER_SIDES {
                let v00 = capsule_ring_vertex(side, latitude0, axial_fraction, axial_sign);
                let v10 = capsule_ring_vertex(side + 1, latitude0, axial_fraction, axial_sign);

                if ring + 1 == CAPSULE_CAP_RINGS {
                    let pole = [0.0, 0.0, axial_fraction, axial_sign];
                    let triangle = if axial_sign < 0.0 {
                        [v00, pole, v10]
                    } else {
                        [v00, v10, pole]
                    };
                    for local in triangle {
                        vertices.push(CylinderVertex { local });
                    }
                    continue;
                }

                let v11 = capsule_ring_vertex(side + 1, latitude1, axial_fraction, axial_sign);
                let v01 = capsule_ring_vertex(side, latitude1, axial_fraction, axial_sign);
                let triangles = if axial_sign < 0.0 {
                    [v00, v11, v10, v00, v01, v11]
                } else {
                    [v00, v10, v11, v00, v11, v01]
                };
                for local in triangles {
                    vertices.push(CylinderVertex { local });
                }
            }
        }
    }

    vertices
}

// ---------------------------------------------------------------------------

/// GPU resources for molecule rendering, stored in egui's `callback_resources`.
pub(crate) struct MoleculeRenderer {
    sphere_pipeline: wgpu::RenderPipeline,
    cylinder_pipeline: wgpu::RenderPipeline,
    mesh_opaque_pipeline: wgpu::RenderPipeline,
    mesh_transparent_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    cylinder_vertices: wgpu::Buffer,
    cylinder_vertex_count: u32,
    spheres: wgpu::Buffer,
    sphere_capacity: u32,
    sphere_count: u32,
    cylinders: wgpu::Buffer,
    cylinder_capacity: u32,
    cylinder_count: u32,
    cartoon: wgpu::Buffer,
    cartoon_capacity: u32,
    cartoon_count: u32,
    surface: wgpu::Buffer,
    surface_capacity: u32,
    surface_count: u32,
    srgb_framebuffer: bool,
}

impl MoleculeRenderer {
    pub(crate) fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("molecule_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("molecule.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("molecule_camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("molecule_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("molecule_camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("molecule_camera_bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        const SPHERE_ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let sphere_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SphereInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &SPHERE_ATTRS,
        };

        const CYL_VERT_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x4];
        let cyl_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CylinderVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &CYL_VERT_ATTRS,
        };
        const CYL_INSTANCE_ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
            1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4
        ];
        let cyl_instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CylinderInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &CYL_INSTANCE_ATTRS,
        };

        let depth_stencil = Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let color_target = wgpu::ColorTargetState {
            format: target_format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        };
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        };

        let sphere_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("molecule_sphere_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("sphere_vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[sphere_layout],
            },
            primitive,
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("sphere_fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(color_target.clone())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let cylinder_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("molecule_cylinder_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("cylinder_vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[cyl_vertex_layout, cyl_instance_layout],
            },
            primitive,
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("cylinder_fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(color_target)],
            }),
            multiview_mask: None,
            cache: None,
        });

        const MESH_ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];
        let mesh_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &MESH_ATTRS,
        };

        // Cartoon ribbons: opaque, depth-writing.
        let mesh_opaque_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("molecule_mesh_opaque_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("mesh_vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: std::slice::from_ref(&mesh_layout),
            },
            primitive,
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("mesh_fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // Molecular surface: translucent, depth-tested but not depth-writing.
        let mesh_transparent_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("molecule_mesh_transparent_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("mesh_vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[mesh_layout],
                },
                primitive,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("mesh_fs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });

        let unit_capsule = unit_capsule_vertices();
        let cylinder_vertices = create_static_buffer(
            device,
            "molecule_unit_capsule",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&unit_capsule),
        );

        let spheres = create_dynamic_buffer::<SphereInstance>(device, "molecule_spheres", 1);
        let cylinders = create_dynamic_buffer::<CylinderInstance>(device, "molecule_cylinders", 1);
        let cartoon = create_dynamic_buffer::<MeshVertex>(device, "molecule_cartoon", 1);
        let surface = create_dynamic_buffer::<MeshVertex>(device, "molecule_surface", 1);

        Self {
            sphere_pipeline,
            cylinder_pipeline,
            mesh_opaque_pipeline,
            mesh_transparent_pipeline,
            camera_buffer,
            camera_bind_group,
            cylinder_vertices,
            cylinder_vertex_count: unit_capsule.len() as u32,
            spheres,
            sphere_capacity: 1,
            sphere_count: 0,
            cylinders,
            cylinder_capacity: 1,
            cylinder_count: 0,
            cartoon,
            cartoon_capacity: 1,
            cartoon_count: 0,
            surface,
            surface_capacity: 1,
            surface_count: 0,
            srgb_framebuffer: target_format.is_srgb(),
        }
    }

    fn write_camera(&self, queue: &wgpu::Queue, mut uniform: CameraUniform) {
        uniform.params[3] = if self.srgb_framebuffer { 1.0 } else { 0.0 };
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &MoleculeInstances,
    ) {
        self.sphere_count = instances.spheres.len() as u32;
        self.cylinder_count = instances.cylinders.len() as u32;
        self.cartoon_count = instances.cartoon.len() as u32;
        self.surface_count = instances.surface.len() as u32;
        if self.sphere_count > self.sphere_capacity {
            self.sphere_capacity = self.sphere_count.next_power_of_two();
            self.spheres = create_dynamic_buffer::<SphereInstance>(
                device,
                "molecule_spheres",
                self.sphere_capacity,
            );
        }
        if self.cylinder_count > self.cylinder_capacity {
            self.cylinder_capacity = self.cylinder_count.next_power_of_two();
            self.cylinders = create_dynamic_buffer::<CylinderInstance>(
                device,
                "molecule_cylinders",
                self.cylinder_capacity,
            );
        }
        if self.cartoon_count > self.cartoon_capacity {
            self.cartoon_capacity = self.cartoon_count.next_power_of_two();
            self.cartoon = create_dynamic_buffer::<MeshVertex>(
                device,
                "molecule_cartoon",
                self.cartoon_capacity,
            );
        }
        if self.surface_count > self.surface_capacity {
            self.surface_capacity = self.surface_count.next_power_of_two();
            self.surface = create_dynamic_buffer::<MeshVertex>(
                device,
                "molecule_surface",
                self.surface_capacity,
            );
        }
        if !instances.spheres.is_empty() {
            queue.write_buffer(&self.spheres, 0, bytemuck::cast_slice(&instances.spheres));
        }
        if !instances.cylinders.is_empty() {
            queue.write_buffer(
                &self.cylinders,
                0,
                bytemuck::cast_slice(&instances.cylinders),
            );
        }
        if !instances.cartoon.is_empty() {
            queue.write_buffer(&self.cartoon, 0, bytemuck::cast_slice(&instances.cartoon));
        }
        if !instances.surface.is_empty() {
            queue.write_buffer(&self.surface, 0, bytemuck::cast_slice(&instances.surface));
        }
    }

    fn draw(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        render_pass.set_bind_group(0, &self.camera_bind_group, &[]);
        // Opaque first (depth-writing): bonds, atoms, cartoon ribbons.
        if self.cylinder_count > 0 {
            render_pass.set_pipeline(&self.cylinder_pipeline);
            render_pass.set_vertex_buffer(0, self.cylinder_vertices.slice(..));
            render_pass.set_vertex_buffer(1, self.cylinders.slice(..));
            render_pass.draw(0..self.cylinder_vertex_count, 0..self.cylinder_count);
        }
        if self.sphere_count > 0 {
            render_pass.set_pipeline(&self.sphere_pipeline);
            render_pass.set_vertex_buffer(0, self.spheres.slice(..));
            render_pass.draw(0..6, 0..self.sphere_count);
        }
        if self.cartoon_count > 0 {
            render_pass.set_pipeline(&self.mesh_opaque_pipeline);
            render_pass.set_vertex_buffer(0, self.cartoon.slice(..));
            render_pass.draw(0..self.cartoon_count, 0..1);
        }
        // Translucent last (depth-tested, no depth write): molecular surface.
        if self.surface_count > 0 {
            render_pass.set_pipeline(&self.mesh_transparent_pipeline);
            render_pass.set_vertex_buffer(0, self.surface.slice(..));
            render_pass.draw(0..self.surface_count, 0..1);
        }
    }
}

fn create_static_buffer(
    device: &wgpu::Device,
    label: &str,
    usage: wgpu::BufferUsages,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .copy_from_slice(bytes);
    buffer.unmap();
    buffer
}

fn create_dynamic_buffer<T>(device: &wgpu::Device, label: &str, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (capacity as usize * std::mem::size_of::<T>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Build the renderer and store it in egui's callback resources. Idempotent —
/// replaces any previous instance.
pub(crate) fn init(render_state: &egui_wgpu::RenderState) {
    let renderer = MoleculeRenderer::new(&render_state.device, render_state.target_format);
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(renderer);
}

/// One frame's worth of molecule rendering. `upload` is `Some` only when the
/// geometry changed; on camera-only frames it is `None` and the persistent GPU
/// buffers are reused.
struct MoleculeCallback {
    camera: CameraUniform,
    upload: Option<MoleculeInstances>,
}

impl egui_wgpu::CallbackTrait for MoleculeCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(renderer) = callback_resources.get_mut::<MoleculeRenderer>() {
            renderer.write_camera(queue, self.camera);
            if let Some(instances) = &self.upload {
                renderer.upload(device, queue, instances);
            }
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(renderer) = callback_resources.get::<MoleculeRenderer>() {
            renderer.draw(render_pass);
        }
    }
}

/// Queue a molecule render into the egui painter for `rect`.
pub(super) fn emit(
    painter: &egui::Painter,
    rect: egui::Rect,
    projector: &Projector,
    upload: Option<MoleculeInstances>,
) {
    let callback = MoleculeCallback {
        camera: camera_uniform(projector),
        upload,
    };
    painter.add(egui_wgpu::Callback::new_paint_callback(rect, callback));
}

#[cfg(test)]
mod tests;
