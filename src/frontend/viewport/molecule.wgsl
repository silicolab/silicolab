// GPU molecular rendering for the viewport.
//
// Atoms are drawn as camera-facing billboard impostors with a ray-sphere
// intersection in the fragment shader; bonds as instanced capsule meshes.
// Both replicate the CPU `Projector` projection exactly so they line up with the
// CPU-drawn unit-cell box, atom labels, and picking, and both reproduce the CPU
// "paper" shading (`ball_stick::shade_surface_color`) so the look is unchanged.

struct Camera {
    // Upper-left 3x3 rotates world -> view space (yaw then pitch). Stored as a
    // mat4 so the column padding matches std140 without manual work.
    rotation: mat4x4<f32>,
    center: vec4<f32>, // world-space view center (xyz)
    params: vec4<f32>, // camera_distance, near_plane, scale, srgb_framebuffer flag
    screen: vec4<f32>, // local_center.x, local_center.y, rect_width, rect_height
};

@group(0) @binding(0) var<uniform> camera: Camera;

// ---------------------------------------------------------------------------
// Shared projection — mirrors `Projector::project`.

fn to_view(world: vec3<f32>) -> vec3<f32> {
    return (camera.rotation * vec4<f32>(world - camera.center.xyz, 0.0)).xyz;
}

fn perspective_factor(view_z: f32) -> f32 {
    return camera.params.x / max(camera.params.x - view_z, camera.params.y);
}

// view-space position -> clip-space xy in [-1,1] relative to the viewport rect.
fn view_to_ndc(view: vec3<f32>) -> vec2<f32> {
    let persp = perspective_factor(view.z);
    let sx = camera.screen.x + view.x * camera.params.z * persp;
    let sy = camera.screen.y - view.y * camera.params.z * persp;
    return vec2<f32>(sx / camera.screen.z * 2.0 - 1.0, 1.0 - sy / camera.screen.w * 2.0);
}

// view-space depth -> [0,1] device depth (nearer == larger view.z == smaller depth).
fn depth_ndc(view_z: f32) -> f32 {
    return clamp(0.5 - view_z / (2.0 * camera.params.x), 0.0, 1.0);
}

// ---------------------------------------------------------------------------
// Shading — mirrors `ball_stick::shade_surface_color` (operates in gamma space).

const PAPER_TINT = vec3<f32>(0.964706, 0.952941, 0.925490);  // (246,243,236)/255
const SHADOW_TINT = vec3<f32>(0.470588, 0.505882, 0.564706); // (120,129,144)/255
const LIGHT_DIR = vec3<f32>(-0.304060, 0.390934, 0.868145);
const HALF_DIR = vec3<f32>(-0.157322, 0.202274, 0.966588);

fn luminance(c: vec3<f32>) -> f32 {
    return c.r * 0.299 + c.g * 0.587 + c.b * 0.114;
}

fn mix3(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return a + (b - a) * clamp(t, 0.0, 1.0);
}

// Base-color-only shading, evaluated per fragment.
fn washed_color(base: vec3<f32>) -> vec3<f32> {
    let lum = luminance(base);
    let neutral = vec3<f32>(lum, lum, min(lum + 6.0 / 255.0, 1.0));
    let desat = mix3(base, neutral, 0.42);
    let softened = mix3(base, desat, 0.34);
    return mix3(softened, PAPER_TINT, 0.14);
}

fn shade(base: vec3<f32>, normal_view: vec3<f32>) -> vec3<f32> {
    let n = normalize(normal_view);
    let diffuse = max(dot(n, LIGHT_DIR), 0.0);
    let rim = pow(1.0 - abs(n.z), 2.0) * 0.10;
    let soft = pow(max(dot(n, HALF_DIR), 0.0), 5.5) * 0.07;
    let washed = washed_color(base);
    let brightness = clamp(0.46 + diffuse * 0.22 + rim * 0.55, 0.0, 1.0);
    var shaded: vec3<f32>;
    if brightness >= 0.5 {
        shaded = mix3(washed, vec3<f32>(1.0), (brightness - 0.5) * 0.42);
    } else {
        let darker = mix3(washed, vec3<f32>(0.0), (0.5 - brightness) * 0.38);
        shaded = mix3(darker, SHADOW_TINT, 0.18);
    }
    return mix3(shaded, PAPER_TINT, soft);
}

// 0-1 linear from 0-1 sRGB gamma (verbatim from egui.wgsl, so molecule colors
// match egui's UI exactly on an sRGB framebuffer).
fn linear_from_gamma_rgb(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

fn frame_color(rgb_gamma: vec3<f32>) -> vec4<f32> {
    if camera.params.w > 0.5 {
        return vec4<f32>(linear_from_gamma_rgb(rgb_gamma), 1.0);
    }
    return vec4<f32>(rgb_gamma, 1.0);
}

struct FragOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// ---------------------------------------------------------------------------
// Sphere impostors (one instanced quad per atom).

struct SphereIn {
    @location(0) pos_radius: vec4<f32>, // xyz center, w radius
    @location(1) color: vec4<f32>,      // rgba gamma 0-1
};

struct SphereOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) offset: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) center_view_z: f32,
    @location(3) radius: f32,
};

@vertex
fn sphere_vs(in: SphereIn, @builtin(vertex_index) vi: u32) -> SphereOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let center = in.pos_radius.xyz;
    let radius = in.pos_radius.w;
    let view = to_view(center);
    let persp = perspective_factor(view.z);
    let ndc_center = view_to_ndc(view);
    let radius_px = radius * camera.params.z * persp;
    let off = corners[vi];
    let ndc = vec2<f32>(
        ndc_center.x + off.x * radius_px / camera.screen.z * 2.0,
        ndc_center.y - off.y * radius_px / camera.screen.w * 2.0,
    );

    var out: SphereOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.offset = off;
    out.color = in.color.rgb;
    out.center_view_z = view.z;
    out.radius = radius;
    return out;
}

@fragment
fn sphere_fs(in: SphereOut) -> FragOut {
    let d2 = dot(in.offset, in.offset);
    if d2 > 1.0 {
        discard;
    }
    let nz = sqrt(1.0 - d2);
    let normal = vec3<f32>(in.offset.x, in.offset.y, nz);
    let surface_view_z = in.center_view_z + nz * in.radius;

    var out: FragOut;
    out.color = frame_color(shade(in.color, normal));
    out.depth = depth_ndc(surface_view_z);
    return out;
}

// ---------------------------------------------------------------------------
// Bond capsules (instanced unit mesh).

struct CylinderIn {
    @location(0) local: vec4<f32>,       // radial xy, length fraction, radius-scaled axial offset/normal
    @location(1) start_len: vec4<f32>,   // instance: xyz start, w length
    @location(2) axis_radius: vec4<f32>, // instance: xyz unit axis, w radius
    @location(3) side_u: vec4<f32>,      // instance: xyz U basis
    @location(4) side_v: vec4<f32>,      // instance: xyz V basis
    @location(5) color_a: vec4<f32>,     // instance: start-half color
    @location(6) color_b: vec4<f32>,     // instance: end-half color
};

struct CylinderOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal_view: vec3<f32>,
    @location(1) color_a: vec3<f32>,
    @location(2) color_b: vec3<f32>,
    @location(3) axial_fraction: f32,
};

@vertex
fn cylinder_vs(in: CylinderIn) -> CylinderOut {
    let radial_normal = in.side_u.xyz * in.local.x + in.side_v.xyz * in.local.y;
    let axial = in.start_len.w * in.local.z + in.axis_radius.w * in.local.w;
    let world = in.start_len.xyz + in.axis_radius.xyz * axial
        + radial_normal * in.axis_radius.w;
    let view = to_view(world);
    let world_normal = normalize(radial_normal + in.axis_radius.xyz * in.local.w);

    var out: CylinderOut;
    out.clip = vec4<f32>(view_to_ndc(view), depth_ndc(view.z), 1.0);
    out.normal_view = (camera.rotation * vec4<f32>(world_normal, 0.0)).xyz;
    out.color_a = in.color_a.rgb;
    out.color_b = in.color_b.rgb;
    out.axial_fraction = in.local.z;
    return out;
}

@fragment
fn cylinder_fs(in: CylinderOut) -> @location(0) vec4<f32> {
    let color = select(in.color_a, in.color_b, in.axial_fraction >= 0.5);
    return frame_color(shade(color, in.normal_view));
}

// ---------------------------------------------------------------------------
// General triangle mesh (cartoon ribbons + molecular surface). Vertices carry a
// world position, world normal, and rgba color; depth uses the shared mapping so
// meshes interpenetrate the spheres/cylinders correctly. Lit two-sided so thin
// ribbons and the inside of a translucent surface stay shaded.

struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal_view: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(linear) barycentric: vec3<f32>,
};

@vertex
fn mesh_vs(in: MeshIn, @builtin(vertex_index) vertex_index: u32) -> MeshOut {
    let view = to_view(in.position);
    var out: MeshOut;
    out.clip = vec4<f32>(view_to_ndc(view), depth_ndc(view.z), 1.0);
    out.normal_view = (camera.rotation * vec4<f32>(in.normal, 0.0)).xyz;
    out.color = in.color;
    let corner = vertex_index % 3u;
    out.barycentric = select(
        select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), corner == 1u),
        vec3<f32>(1.0, 0.0, 0.0),
        corner == 0u,
    );
    return out;
}

@fragment
fn mesh_wire_fs(in: MeshOut) -> @location(0) vec4<f32> {
    let nearest_edge = min(in.barycentric.x, min(in.barycentric.y, in.barycentric.z));
    let width = max(fwidth(nearest_edge) * 1.35, 0.00001);
    let coverage = 1.0 - smoothstep(0.0, width, nearest_edge);
    if coverage <= 0.01 {
        discard;
    }
    var n = in.normal_view;
    if n.z < 0.0 {
        n = -n;
    }
    let lit = frame_color(shade(in.color.rgb, n));
    return vec4<f32>(lit.rgb, in.color.a * coverage);
}

@fragment
fn mesh_fs(in: MeshOut) -> @location(0) vec4<f32> {
    var n = in.normal_view;
    if n.z < 0.0 {
        n = -n;
    }
    let lit = frame_color(shade(in.color.rgb, n));
    return vec4<f32>(lit.rgb, in.color.a);
}
