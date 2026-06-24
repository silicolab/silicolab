//! GPU-accelerated molecular rendering.
//!
//! Atoms are drawn as instanced billboard sphere impostors and bonds as
//! instanced low-poly cylinders, all in a single egui paint callback that draws
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

/// Sides of the unit bond cylinder. Bonds are thin, so a coarse ring reads as
/// round; ends are left open because the atom spheres cover them.
const CYLINDER_SIDES: usize = 10;

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
    /// rx, ry, y, cap (unused here; sides only).
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

fn unit_cylinder_vertices() -> Vec<CylinderVertex> {
    use std::f32::consts::TAU;
    let mut vertices = Vec::with_capacity(CYLINDER_SIDES * 6);
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

        let unit_cylinder = unit_cylinder_vertices();
        let cylinder_vertices = create_static_buffer(
            device,
            "molecule_unit_cylinder",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&unit_cylinder),
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
            cylinder_vertex_count: unit_cylinder.len() as u32,
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
mod tests {
    use super::super::camera::view_center_and_radius;
    use super::super::render::{build_molecule_instances, build_surface_world_mesh};
    use super::super::{SurfaceCache, SurfaceCacheKey};
    use super::*;
    use crate::domain::biopolymer::{
        PdbAtomAnnotation, ResidueId, SecondaryStructureKind, SecondaryStructureSpan,
        build_biopolymer,
    };
    use crate::domain::{Atom, Bond, BondType, Structure};
    use crate::frontend::{AtomSelection, ViewportVisualState};
    use eframe::egui::{Pos2, Rect, Vec2};
    use nalgebra::Point3;

    fn test_projector() -> Projector {
        Projector::new(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
            Point3::new(1.0, 2.0, 3.0),
            12.0,
            500.0,
            0.6,
            -0.3,
            Vec2::new(10.0, -5.0),
        )
    }

    /// Reimplements the WGSL projection from the uniform and checks it matches
    /// `Projector::project` (converted to rect-relative NDC). Guards the shader
    /// transcription, which can't be exercised without a GPU.
    fn mirror_ndc(uniform: &CameraUniform, world: Point3<f32>) -> [f32; 2] {
        let w = [
            world.x - uniform.center[0],
            world.y - uniform.center[1],
            world.z - uniform.center[2],
        ];
        let col = uniform.rotation;
        let view = [
            col[0][0] * w[0] + col[1][0] * w[1] + col[2][0] * w[2],
            col[0][1] * w[0] + col[1][1] * w[1] + col[2][1] * w[2],
            col[0][2] * w[0] + col[1][2] * w[1] + col[2][2] * w[2],
        ];
        let (cd, near, scale) = (uniform.params[0], uniform.params[1], uniform.params[2]);
        let persp = cd / (cd - view[2]).max(near);
        let sx = uniform.screen[0] + view[0] * scale * persp;
        let sy = uniform.screen[1] - view[1] * scale * persp;
        [
            sx / uniform.screen[2] * 2.0 - 1.0,
            1.0 - sy / uniform.screen[3] * 2.0,
        ]
    }

    #[test]
    fn camera_uniform_projection_matches_cpu_projector() {
        let projector = test_projector();
        let uniform = camera_uniform(&projector);
        let rect = projector.rect;
        for world in [
            Point3::new(1.0, 2.0, 3.0),
            Point3::new(5.0, -4.0, 9.0),
            Point3::new(-7.0, 8.0, -2.0),
            Point3::new(0.0, 0.0, 0.0),
        ] {
            let projected = projector.project(world);
            let expected = [
                (projected.pos.x - rect.min.x) / rect.width() * 2.0 - 1.0,
                1.0 - (projected.pos.y - rect.min.y) / rect.height() * 2.0,
            ];
            let actual = mirror_ndc(&uniform, world);
            assert!(
                (expected[0] - actual[0]).abs() < 1e-4 && (expected[1] - actual[1]).abs() < 1e-4,
                "world {world:?}: cpu {expected:?} vs shader-mirror {actual:?}"
            );
        }
    }

    #[test]
    fn unit_cylinder_is_two_triangles_per_side() {
        assert_eq!(unit_cylinder_vertices().len(), CYLINDER_SIDES * 6);
    }

    fn benzene() -> Structure {
        let coords = [
            ("C", 1.396, 0.000),
            ("C", 0.698, 1.209),
            ("C", -0.698, 1.209),
            ("C", -1.396, 0.000),
            ("C", -0.698, -1.209),
            ("C", 0.698, -1.209),
            ("H", 2.479, 0.000),
            ("H", 1.240, 2.147),
            ("H", -1.240, 2.147),
            ("H", -2.479, 0.000),
            ("H", -1.240, -2.147),
            ("H", 1.240, -2.147),
        ];
        let atoms = coords
            .iter()
            .map(|(element, x, y)| Atom {
                element: (*element).to_string(),
                position: Point3::new(*x, *y, 0.0),
                charge: 0.0,
            })
            .collect();
        let mut bonds = Vec::new();
        for ring in 0..6 {
            bonds.push(Bond::with_type(ring, (ring + 1) % 6, BondType::Aromatic));
            bonds.push(Bond::with_type(ring, ring + 6, BondType::Single));
        }
        Structure::with_bonds("benzene", atoms, bonds)
    }

    /// Renders a molecule to an offscreen RGBA target on a real GPU and returns
    /// the pixels (row-unpadded). Mirrors the egui-pass setup: a `Depth32Float`
    /// attachment cleared to 1.0, color cleared to the background.
    fn render_offscreen(
        instances: &MoleculeInstances,
        projector: &Projector,
        width: u32,
        height: u32,
        background: [f32; 4],
    ) -> Vec<u8> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("request adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("request device");

        let mut renderer = MoleculeRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        renderer.write_camera(&queue, camera_uniform(projector));
        renderer.upload(&device, &queue, instances);

        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen_color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen_depth"),
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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offscreen_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: background[0] as f64,
                            g: background[1] as f64,
                            b: background[2] as f64,
                            a: 1.0,
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

        let bytes_per_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen_readback"),
            size: (bytes_per_row * height) as u64,
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

        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let data = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        pixels
    }

    /// GPU smoke test: render benzene and save a PNG for visual inspection.
    /// Ignored by default because it needs a usable GPU adapter; run with
    /// `cargo test --release -- --ignored gpu_renders_benzene`.
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn gpu_renders_benzene_to_png() {
        let structure = benzene();
        let instances = build_molecule_instances(
            &structure,
            &AtomSelection::default(),
            &ViewportVisualState::default(),
        );
        assert_eq!(instances.spheres.len(), 12);
        // 6 aromatic ring bonds (each a main cylinder + inner dashes) plus 6
        // single C-H bonds, so well more than one cylinder per bond.
        assert!(
            instances.cylinders.len() > 12,
            "aromatic bonds should add inner dashes, got {}",
            instances.cylinders.len()
        );

        let (width, height) = (900u32, 700u32);
        let (center, radius) = view_center_and_radius(&structure, false);
        let projector = Projector::new(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32)),
            center,
            width.min(height) as f32 * 0.35 / radius,
            radius * 3.2,
            0.6,
            0.45,
            Vec2::ZERO,
        );
        let background = ViewportVisualState::default()
            .background_color
            .to_normalized_gamma_f32();
        let pixels = render_offscreen(&instances, &projector, width, height, background);

        // The molecule must have drawn something other than the background.
        let bg = [
            (background[0] * 255.0) as u8,
            (background[1] * 255.0) as u8,
            (background[2] * 255.0) as u8,
        ];
        let drawn = pixels
            .chunks_exact(4)
            .filter(|p| {
                p[0].abs_diff(bg[0]) > 8 || p[1].abs_diff(bg[1]) > 8 || p[2].abs_diff(bg[2]) > 8
            })
            .count();
        assert!(drawn > 1000, "expected molecule pixels, got {drawn}");

        let path = std::path::Path::new("target").join("gpu_smoke_benzene.png");
        image::RgbaImage::from_raw(width, height, pixels)
            .expect("image buffer")
            .save(&path)
            .expect("save png");
        eprintln!("wrote {}", path.display());
    }

    /// A synthetic single-chain protein: an idealized α-helix followed by an
    /// extended β-strand, so the cartoon renders a helix ribbon, a coil turn, and
    /// a sheet arrow.
    fn helix_sheet_protein() -> Structure {
        // Cα trace: an idealized α-helix (100°/residue, 1.5 Å rise) followed by an
        // extended β-strand continuing from the helix end, so the chain is one
        // continuous backbone.
        let mut ca = Vec::new();
        for i in 0..11usize {
            let angle = i as f32 * 1.745;
            ca.push(Point3::new(
                2.3 * angle.cos(),
                2.3 * angle.sin(),
                1.5 * i as f32,
            ));
        }
        let base = *ca.last().unwrap();
        for i in 0..9usize {
            let step = i as f32 + 1.0;
            ca.push(Point3::new(
                base.x + 3.3 * step,
                base.y + if i % 2 == 0 { 0.5 } else { -0.5 },
                base.z,
            ));
        }

        // Give each residue a full N/CA/C backbone; N and C sit on the Cα–Cα
        // midpoints so consecutive C(i)/N(i+1) coincide (a zero-length peptide
        // bond), keeping the chain a single contiguous ribbon.
        let mut atoms = Vec::with_capacity(ca.len() * 3);
        let mut annotations = Vec::with_capacity(ca.len() * 3);
        for (index, &position) in ca.iter().enumerate() {
            let previous = if index > 0 { ca[index - 1] } else { position };
            let next = if index + 1 < ca.len() {
                ca[index + 1]
            } else {
                position
            };
            let nitrogen = Point3::from((previous.coords + position.coords) * 0.5);
            let carbon = Point3::from((position.coords + next.coords) * 0.5);
            for (atom_name, element, atom_position) in [
                ("N", "N", nitrogen),
                ("CA", "C", position),
                ("C", "C", carbon),
            ] {
                atoms.push(Atom {
                    element: element.to_string(),
                    position: atom_position,
                    charge: 0.0,
                });
                annotations.push(PdbAtomAnnotation {
                    atom_name: atom_name.to_string(),
                    residue_name: "ALA".to_string(),
                    chain_id: 'A',
                    residue_seq: index as i32 + 1,
                    insertion_code: ' ',
                });
            }
        }

        let spans = vec![
            SecondaryStructureSpan {
                kind: SecondaryStructureKind::Helix,
                start: ResidueId::new('A', 1, ' '),
                end: ResidueId::new('A', 11, ' '),
            },
            SecondaryStructureSpan {
                kind: SecondaryStructureKind::Sheet,
                start: ResidueId::new('A', 12, ' '),
                end: ResidueId::new('A', 20, ' '),
            },
        ];
        let biopolymer = build_biopolymer(&annotations, spans).expect("biopolymer");
        Structure {
            title: "helix_sheet".to_string(),
            atoms,
            bonds: Vec::new(),
            cell: None,
            biopolymer: Some(biopolymer),
        }
    }

    /// GPU smoke test: render a helix+sheet cartoon and save a PNG so the ribbon
    /// cross-sections (flat helix, sheet arrow, coil tube) can be inspected.
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn gpu_renders_cartoon_to_png() {
        let structure = helix_sheet_protein();
        let instances = build_molecule_instances(
            &structure,
            &AtomSelection::default(),
            &ViewportVisualState::default(),
        );
        assert!(
            instances.cartoon.len() > 1000,
            "expected a cartoon mesh, got {} vertices",
            instances.cartoon.len()
        );

        let (width, height) = (1000u32, 720u32);
        let (center, radius) = view_center_and_radius(&structure, false);
        let projector = Projector::new(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32)),
            center,
            width.min(height) as f32 * 0.6 / radius,
            radius * 3.2,
            0.7,
            0.5,
            Vec2::ZERO,
        );
        let background = ViewportVisualState::default()
            .background_color
            .to_normalized_gamma_f32();
        let pixels = render_offscreen(&instances, &projector, width, height, background);

        let path = std::path::Path::new("target").join("gpu_smoke_cartoon.png");
        image::RgbaImage::from_raw(width, height, pixels)
            .expect("image buffer")
            .save(&path)
            .expect("save png");
        eprintln!("wrote {}", path.display());
    }

    /// GPU smoke test: render a cartoon under a translucent molecular surface,
    /// exercising the transparent mesh pipeline and alpha blending.
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn gpu_renders_surface_to_png() {
        let structure = helix_sheet_protein();
        let mut visual_state = ViewportVisualState::default();
        visual_state.surface.chains.insert('A');
        visual_state.surface.transparency = 0.45;

        let mut instances =
            build_molecule_instances(&structure, &AtomSelection::default(), &visual_state);
        let surface_key = SurfaceCacheKey::new(1, 1, &structure, &visual_state);
        instances.surface = build_surface_world_mesh(
            &structure,
            &surface_key,
            &visual_state,
            &mut SurfaceCache::default(),
        );
        assert!(
            !instances.surface.is_empty(),
            "expected a surface mesh for chain A"
        );

        let (width, height) = (1000u32, 720u32);
        let (center, radius) = view_center_and_radius(&structure, false);
        let projector = Projector::new(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32)),
            center,
            width.min(height) as f32 * 0.6 / radius,
            radius * 3.2,
            0.7,
            0.5,
            Vec2::ZERO,
        );
        let background = visual_state.background_color.to_normalized_gamma_f32();
        let pixels = render_offscreen(&instances, &projector, width, height, background);

        let path = std::path::Path::new("target").join("gpu_smoke_surface.png");
        image::RgbaImage::from_raw(width, height, pixels)
            .expect("image buffer")
            .save(&path)
            .expect("save png");
        eprintln!("wrote {}", path.display());
    }
}
