use eframe::egui::{Pos2, Rect, Vec2};
use nalgebra::{Point3, Vector3};

use crate::domain::Structure;

#[derive(Clone, Copy, Default, PartialEq)]
pub struct ViewCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub pan: Vec2,
    pub zoom: f32,
}

pub(super) struct Projector {
    pub(super) rect: Rect,
    pub(super) center: Point3<f32>,
    pub(super) scale: f32,
    pub(super) camera_distance: f32,
    pub(super) pan: Vec2,
    /// Yaw/pitch sines and cosines, computed once at construction. The viewport
    /// projects and shades hundreds of surface vertices per atom; recomputing
    /// `sin_cos` per vertex (as the bare [`rotate`] does) dominated the hot path.
    sin_yaw: f32,
    cos_yaw: f32,
    sin_pitch: f32,
    cos_pitch: f32,
}

#[derive(Clone, Copy)]
pub(super) struct Projected {
    pub(super) pos: Pos2,
    pub(super) depth: f32,
    pub(super) scale: f32,
}

impl Projector {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        rect: Rect,
        center: Point3<f32>,
        scale: f32,
        camera_distance: f32,
        yaw: f32,
        pitch: f32,
        pan: Vec2,
    ) -> Self {
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        Self {
            rect,
            center,
            scale,
            camera_distance,
            pan,
            sin_yaw,
            cos_yaw,
            sin_pitch,
            cos_pitch,
        }
    }

    /// Rotate a world-space vector into view space using the camera's cached
    /// yaw/pitch trig — a yaw-then-pitch rotation with no per-call `sin_cos`.
    pub(super) fn rotate_to_view(&self, v: Vector3<f32>) -> Vector3<f32> {
        rotate_with(
            v,
            self.sin_yaw,
            self.cos_yaw,
            self.sin_pitch,
            self.cos_pitch,
        )
    }

    pub(super) fn view_space(&self, point: Point3<f32>) -> Vector3<f32> {
        self.rotate_to_view(point - self.center)
    }

    pub(super) fn project(&self, point: Point3<f32>) -> Projected {
        let rotated = self.view_space(point);
        let near_plane = (self.camera_distance * 0.2).max(0.1);
        let perspective = self.camera_distance / (self.camera_distance - rotated.z).max(near_plane);
        let screen_center = self.rect.center() + self.pan;

        Projected {
            pos: Pos2::new(
                screen_center.x + rotated.x * self.scale * perspective,
                screen_center.y - rotated.y * self.scale * perspective,
            ),
            depth: rotated.z,
            scale: perspective,
        }
    }
}

pub(crate) fn view_center_and_radius(
    structure: &Structure,
    include_cell: bool,
) -> (Point3<f32>, f32) {
    if include_cell {
        return (structure.center(), structure.radius().max(1.0));
    }
    if structure.atoms.is_empty() {
        return (Point3::origin(), 1.0);
    }

    let sum = structure
        .atoms
        .iter()
        .fold(Vector3::zeros(), |acc, atom| acc + atom.position.coords);
    let center = Point3::from(sum / structure.atoms.len() as f32);
    let radius = structure
        .atoms
        .iter()
        .map(|atom| nalgebra::distance(&center, &atom.position))
        .fold(1.0_f32, f32::max);
    (center, radius)
}

/// Yaw-then-pitch rotation with the trig terms supplied by the caller, so a
/// per-frame `sin_cos` can be shared across many vertices.
fn rotate_with(v: Vector3<f32>, sy: f32, cy: f32, sp: f32, cp: f32) -> Vector3<f32> {
    let x = cy * v.x + sy * v.z;
    let z = -sy * v.x + cy * v.z;
    let y = cp * v.y - sp * z;
    let z = sp * v.y + cp * z;

    Vector3::new(x, y, z)
}
