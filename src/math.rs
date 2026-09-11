//! Minimal linear-algebra core, hand-rolled so the engine carries zero math
//! dependencies to drift out from under it.
//! Conventions: column-major storage, right-handed view space (camera looks
//! down -Z), depth range [0,1] — exactly wgpu / WebGPU clip space.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const UP: Vec3 = Vec3 { x: 0.0, y: 1.0, z: 0.0 };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }
    pub fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn scale(self, k: f32) -> Vec3 {
        Vec3::new(self.x * k, self.y * k, self.z * k)
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalized(self) -> Vec3 {
        let l = self.len();
        if l < 1e-8 {
            Vec3::ZERO
        } else {
            self.scale(1.0 / l)
        }
    }
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        self.add(o.sub(self).scale(t))
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Mat4 {
    /// Column-major: m[col * 4 + row].
    pub m: [f32; 16],
}

impl Mat4 {
    pub const fn identity() -> Self {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Mat4 { m }
    }

    pub fn translation(x: f32, y: f32, z: f32) -> Self {
        let mut r = Mat4::identity();
        r.m[12] = x;
        r.m[13] = y;
        r.m[14] = z;
        r
    }

    /// Rotation about the X axis (column-major layout of the standard matrix).
    pub fn rotation_x(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        let mut r = Mat4::identity();
        r.m[5] = c;
        r.m[6] = s;
        r.m[9] = -s;
        r.m[10] = c;
        r
    }

    /// self * other (apply `other` first, then `self`).
    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut r = [0.0f32; 16];
        for c in 0..4 {
            for rw in 0..4 {
                r[c * 4 + rw] = self.m[rw] * o.m[c * 4]
                    + self.m[4 + rw] * o.m[c * 4 + 1]
                    + self.m[8 + rw] * o.m[c * 4 + 2]
                    + self.m[12 + rw] * o.m[c * 4 + 3];
            }
        }
        Mat4 { m: r }
    }

    /// Transform a point (assumes the bottom row is [0,0,0,1], as with our
    /// rigid transforms) and return the resulting xyz triple.
    pub fn transform_point(&self, x: f32, y: f32, z: f32) -> [f32; 3] {
        let m = &self.m;
        [
            m[0] * x + m[4] * y + m[8] * z + m[12],
            m[1] * x + m[5] * y + m[9] * z + m[13],
            m[2] * x + m[6] * y + m[10] * z + m[14],
        ]
    }

    /// Right-handed perspective with depth mapped to [0,1] (wgpu clip space).
    pub fn perspective_rh_zo(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / (fov_y_radians / 2.0).tan();
        let mut m = [0.0f32; 16];
        m[0] = f / aspect;
        m[5] = f;
        m[10] = far / (near - far);
        m[11] = -1.0;
        m[14] = (near * far) / (near - far);
        Mat4 { m }
    }

    /// Classic right-handed look-at. Camera looks down -Z.
    pub fn look_at_rh(eye: Vec3, at: Vec3, up: Vec3) -> Mat4 {
        let zax = eye.sub(at).normalized();
        let xax = up.cross(zax).normalized();
        let yax = zax.cross(xax);
        let mut m = [0.0f32; 16];
        m[0] = xax.x;
        m[1] = yax.x;
        m[2] = zax.x;
        m[4] = xax.y;
        m[5] = yax.y;
        m[6] = zax.y;
        m[8] = xax.z;
        m[9] = yax.z;
        m[10] = zax.z;
        m[12] = -xax.dot(eye);
        m[13] = -yax.dot(eye);
        m[14] = -zax.dot(eye);
        m[15] = 1.0;
        Mat4 { m }
    }

    /// Column extraction, matching the WGSL mat4x4 uniform layout.
    pub fn cols(&self) -> [[f32; 4]; 4] {
        let m = &self.m;
        [
            [m[0], m[1], m[2], m[3]],
            [m[4], m[5], m[6], m[7]],
            [m[8], m[9], m[10], m[11]],
            [m[12], m[13], m[14], m[15]],
        ]
    }
}
