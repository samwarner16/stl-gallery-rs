use glam::{Mat4, Vec3};

#[derive(Copy, Clone, Debug)]
pub enum View {
    // 6 cardinal — face centers of the unit cube.
    Front, Back, Left, Right, Top, Bottom,
    // 8 isometric — vertices of the unit cube.
    IsoTopFrontRight,    IsoTopFrontLeft,    IsoTopBackRight,    IsoTopBackLeft,
    IsoBottomFrontRight, IsoBottomFrontLeft, IsoBottomBackRight, IsoBottomBackLeft,
}

impl View {
    pub fn all() -> &'static [View] {
        &[
            View::Front, View::Back, View::Left, View::Right, View::Top, View::Bottom,
            View::IsoTopFrontRight,    View::IsoTopFrontLeft,
            View::IsoTopBackRight,     View::IsoTopBackLeft,
            View::IsoBottomFrontRight, View::IsoBottomFrontLeft,
            View::IsoBottomBackRight,  View::IsoBottomBackLeft,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            View::Front  => "front",
            View::Back   => "back",
            View::Left   => "left",
            View::Right  => "right",
            View::Top    => "top",
            View::Bottom => "bottom",
            View::IsoTopFrontRight    => "iso_top_front_right",
            View::IsoTopFrontLeft     => "iso_top_front_left",
            View::IsoTopBackRight     => "iso_top_back_right",
            View::IsoTopBackLeft      => "iso_top_back_left",
            View::IsoBottomFrontRight => "iso_bottom_front_right",
            View::IsoBottomFrontLeft  => "iso_bottom_front_left",
            View::IsoBottomBackRight  => "iso_bottom_back_right",
            View::IsoBottomBackLeft   => "iso_bottom_back_left",
        }
    }

    /// Direction from the model's origin toward the camera. The model is
    /// normalized to a unit-radius region around the origin, so this is
    /// scaled by a fixed distance to obtain the eye position.
    fn eye_direction(&self) -> Vec3 {
        match self {
            View::Front  => Vec3::new( 0.0,  0.0,  1.0),
            View::Back   => Vec3::new( 0.0,  0.0, -1.0),
            View::Right  => Vec3::new( 1.0,  0.0,  0.0),
            View::Left   => Vec3::new(-1.0,  0.0,  0.0),
            View::Top    => Vec3::new( 0.0,  1.0,  0.0),
            View::Bottom => Vec3::new( 0.0, -1.0,  0.0),
            View::IsoTopFrontRight    => Vec3::new( 1.0,  1.0,  1.0).normalize(),
            View::IsoTopFrontLeft     => Vec3::new(-1.0,  1.0,  1.0).normalize(),
            View::IsoTopBackRight     => Vec3::new( 1.0,  1.0, -1.0).normalize(),
            View::IsoTopBackLeft      => Vec3::new(-1.0,  1.0, -1.0).normalize(),
            View::IsoBottomFrontRight => Vec3::new( 1.0, -1.0,  1.0).normalize(),
            View::IsoBottomFrontLeft  => Vec3::new(-1.0, -1.0,  1.0).normalize(),
            View::IsoBottomBackRight  => Vec3::new( 1.0, -1.0, -1.0).normalize(),
            View::IsoBottomBackLeft   => Vec3::new(-1.0, -1.0, -1.0).normalize(),
        }
    }

    /// Pick an `up` that isn't colinear with the eye direction. Top/Bottom
    /// would otherwise produce a degenerate look_at; everything else uses
    /// world +Y so the model's vertical axis stays vertical on screen.
    fn up_vector(&self) -> Vec3 {
        match self {
            View::Top    => Vec3::NEG_Z,
            View::Bottom => Vec3::Z,
            _            => Vec3::Y,
        }
    }
}

/// Returns `proj * view`. Camera frames a unit sphere; `distance` controls how
/// much of the frame the model occupies (smaller = closer = larger on screen).
pub fn view_projection_matrix(view: View, aspect: f32, distance: f32) -> Mat4 {
    let eye    = view.eye_direction() * distance;
    let target = Vec3::ZERO;
    let up     = view.up_vector();

    let v = Mat4::look_at_rh(eye, target, up);
    // wgpu uses 0..1 NDC depth (Vulkan/DX/Metal style), so the standard
    // perspective_rh from glam works directly — no GL-style depth remap needed.
    let p = Mat4::perspective_rh(45_f32.to_radians(), aspect, 0.1, 100.0);
    p * v
}
