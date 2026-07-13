use super::super::camera::view_center_and_radius;
use super::super::render::{build_molecule_instances, build_surface_world_mesh};
use super::super::{SurfaceCache, SurfaceCacheKey};
use super::*;
use crate::domain::biopolymer::{
    PdbAtomAnnotation, ResidueId, SecondaryStructureKind, SecondaryStructureSpan, build_biopolymer,
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
fn unit_capsule_has_side_wall_and_hemisphere_caps() {
    let vertices = unit_capsule_vertices();
    let cap_vertices_per_side = (CAPSULE_CAP_RINGS - 1) * 6 + 3;
    assert_eq!(
        vertices.len(),
        CYLINDER_SIDES * (6 + cap_vertices_per_side * 2)
    );

    let mut min_axial = f32::INFINITY;
    let mut max_axial = f32::NEG_INFINITY;
    for vertex in vertices {
        let [x, y, fraction, axial_offset] = vertex.local;
        let normal_length_squared = x * x + y * y + axial_offset * axial_offset;
        assert!((normal_length_squared - 1.0).abs() < 1e-5);
        assert!((0.0..=1.0).contains(&fraction));
        min_axial = min_axial.min(fraction + axial_offset);
        max_axial = max_axial.max(fraction + axial_offset);
    }

    assert!((min_axial + 1.0).abs() < 1e-6);
    assert!((max_axial - 2.0).abs() < 1e-6);
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
