use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal:   [f32; 3],
}

#[derive(Copy, Clone, Debug)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    pub fn longest_extent(&self) -> f32 {
        let dx = self.max[0] - self.min[0];
        let dy = self.max[1] - self.min[1];
        let dz = self.max[2] - self.min[2];
        dx.max(dy).max(dz)
    }
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices:  Vec<u32>,
    pub bounds:   Bounds,
}

pub fn load_stl(path: &Path) -> Result<Mesh> {
    let mut reader = BufReader::new(File::open(path).context("opening STL")?);
    let stl = stl_io::read_stl(&mut reader).context("parsing STL")?;

    let mut vertices = Vec::with_capacity(stl.faces.len() * 3);
    let mut indices  = Vec::with_capacity(stl.faces.len() * 3);

    let mut min = [f32::INFINITY;     3];
    let mut max = [f32::NEG_INFINITY; 3];

    // Expand to a non-indexed list (one Vertex per corner) so we can carry
    // per-face flat normals — STL has no shared per-vertex normals anyway.
    for (i, face) in stl.faces.iter().enumerate() {
        let normal = [face.normal[0], face.normal[1], face.normal[2]];
        for &vid in &face.vertices {
            let v = stl.vertices[vid];
            let position = [v[0], v[1], v[2]];
            for k in 0..3 {
                min[k] = min[k].min(position[k]);
                max[k] = max[k].max(position[k]);
            }
            vertices.push(Vertex { position, normal });
        }
        let base = (i * 3) as u32;
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    Ok(Mesh { vertices, indices, bounds: Bounds { min, max } })
}
