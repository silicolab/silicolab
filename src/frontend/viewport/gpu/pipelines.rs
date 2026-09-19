use super::{CylinderInstance, CylinderVertex, DEPTH_FORMAT, MeshVertex, SphereInstance};
use eframe::wgpu;

pub(super) struct Pipelines {
    pub(super) sphere_pipeline: wgpu::RenderPipeline,
    pub(super) cylinder_pipeline: wgpu::RenderPipeline,
    pub(super) cylinder_outline_pipeline: wgpu::RenderPipeline,
    pub(super) mesh_opaque_pipeline: wgpu::RenderPipeline,
    pub(super) mesh_transparent_pipeline: wgpu::RenderPipeline,
    pub(super) mesh_wire_pipeline: wgpu::RenderPipeline,
}

pub(super) fn create(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> Pipelines {
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
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("sphere_vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[sphere_layout],
        },
        primitive,
        depth_stencil: depth_stencil.clone(),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("sphere_fs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(color_target.clone())],
        }),
        multiview_mask: None,
        cache: None,
    });

    let cylinder_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("molecule_cylinder_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("cylinder_vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[cyl_vertex_layout.clone(), cyl_instance_layout.clone()],
        },
        primitive,
        depth_stencil: depth_stencil.clone(),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("cylinder_fs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(color_target.clone())],
        }),
        multiview_mask: None,
        cache: None,
    });

    let cylinder_outline_pipeline =
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("molecule_cylinder_outline_pipeline"),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("cylinder_outline_vs"),
                compilation_options: Default::default(),
                buffers: &[cyl_vertex_layout, cyl_instance_layout],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Front),
                ..primitive
            },
            depth_stencil,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("outline_fs"),
                compilation_options: Default::default(),
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
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
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
            module: shader,
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
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("mesh_vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: std::slice::from_ref(&mesh_layout),
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
                module: shader,
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

    let mesh_wire_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("molecule_mesh_wire_pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("mesh_vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: std::slice::from_ref(&mesh_layout),
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..primitive
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("mesh_wire_fs"),
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

    Pipelines {
        sphere_pipeline,
        cylinder_pipeline,
        cylinder_outline_pipeline,
        mesh_opaque_pipeline,
        mesh_transparent_pipeline,
        mesh_wire_pipeline,
    }
}
