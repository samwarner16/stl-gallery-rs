# stl-gallery-rs

A headless command-line tool that renders an STL mesh to a 56-image PNG gallery
covering 14 camera angles (one per face center and one per vertex of an
axis-aligned cube), two viewing distances, and two visual styles (shaded and
edge-detected). Cross-platform via wgpu — Metal on macOS, Vulkan on Linux,
DX12 on Windows. No window required.

```
stl-gallery-rs -i model.stl -o gallery
```

## What you get

For each input STL the program writes 56 PNGs into the output directory.
The 14 angles are: 6 cardinal (face centers of the unit cube) plus 8
isometric (vertices of the unit cube — top/bottom × front/back × left/right):

```
gallery/
├── front.png   back.png   left.png   right.png   top.png   bottom.png
├── iso_top_front_right.png      iso_top_front_left.png
├── iso_top_back_right.png       iso_top_back_left.png
├── iso_bottom_front_right.png   iso_bottom_front_left.png
├── iso_bottom_back_right.png    iso_bottom_back_left.png
└── … (each of the 14 also exists as _far.png, _edges.png, _far_edges.png)
```

Naming convention:

| suffix         | meaning                                      |
|----------------|----------------------------------------------|
| (none)         | shaded, near framing (model fills the frame) |
| `_far`         | shaded, stepped-back framing                 |
| `_edges`       | edge-detected, near framing                  |
| `_far_edges`   | edge-detected, stepped-back framing          |

The shaded variant uses a soft Lambertian light. The edge variant overlays
black lines on flat white faces — useful for technical illustration and for
making mesh topology readable.

## Quick start

You need a recent stable Rust toolchain (1.80+). On macOS, no extra setup
beyond Xcode command-line tools is required (Metal ships with the OS).

```sh
git clone <this repo>
cd stl-gallery-rs
cargo build --release
./target/release/stl-gallery-rs -i path/to/model.stl -o gallery
```

Both ASCII and binary STL files are supported. The model is auto-centered
and uniformly scaled to fit the viewing volume, so any units / origin work
out of the box.

## CLI

```
stl-gallery-rs [OPTIONS] --input <FILE>

Options:
  -i, --input <FILE>          Input STL file (binary or ASCII)
  -o, --output <DIR>          Output directory for PNGs [default: gallery]
      --width <PIXELS>        Render width  [default: 1024]
      --height <PIXELS>       Render height [default: 1024]
      --encoders <N>          PNG-encoder worker threads
                              [default: clamp(num_cpus, 2, 8)]
  -h, --help                  Print help
  -V, --version               Print version
```

Examples:

```sh
# Default 1024×1024, output to ./gallery
stl-gallery-rs -i widget.stl

# Higher resolution, custom output dir
stl-gallery-rs -i widget.stl -o renders --width 2048 --height 2048

# Force a specific encoder count (useful on shared hardware)
stl-gallery-rs -i widget.stl --encoders 4
```

## How it works

### Pipeline overview

```
   STL on disk
        │
        ▼
   ┌─────────┐    ┌────────────────┐    ┌────────────────┐
   │ parser  │───►│   GPU loop     │───►│ encoder pool   │
   │ (CPU)   │    │  (main thread) │    │  (N threads)   │
   └─────────┘    └────────────────┘    └────────────────┘
                          │                      │
                  raw RGBA Vec<u8>          PNG files
                  via channel
```

1. **Parse** (`src/parser.rs`). Read the STL with `stl_io`, expand to a
   non-indexed vertex+normal list (one `Vertex` per triangle corner so the
   GPU can use STL's flat per-face normals). Compute the AABB.

2. **Initialize GPU once** (`Renderer::new` in `src/renderer.rs`):
   - Request a high-performance adapter with `compatible_surface: None`
     (headless — no swapchain, no window).
   - Upload vertex/index/uniform buffers.
   - Compile three pipelines: shaded, normal-G-buffer, post-composite.
   - Allocate render targets: color (`Rgba8UnormSrgb`), normal G-buffer
     (`Rgba16Float`), depth (`Depth32Float`), and a row-aligned readback
     buffer.

3. **Auto-normalize the model**. Translate the mesh so its AABB center is
   at the origin, then uniformly scale so the longest axis fits in `[-1, 1]`.
   Camera framing in `src/camera.rs` is then independent of model units.

4. **Render the gallery**. For each (angle, distance, mode) tuple:
   - Build the view-projection matrix.
   - Run the appropriate render passes (see "Two render modes" below).
   - Copy the color texture to the readback buffer.
   - Map the buffer, strip 256-byte row padding, ship the raw RGBA `Vec<u8>`
     to a worker thread via a bounded channel.

5. **Encode PNGs in parallel**. N worker threads pull `(pixels, path)`
   tuples and run PNG deflate independently. The GPU loop renders the next
   frame while previous frames are still encoding.

### Two render modes

**Plain mode.** One render pass: a vertex shader transforms positions by
`model · view_proj`; a fragment shader applies a single directional light
plus an ambient term to a near-white base color. Cull mode is back-face,
front winding is CCW.

**Edges mode.** Two render passes:

1. *Geometry pass* — same vertex transform, but the fragment shader writes
   the normalized world-space normal into the G-buffer (`Rgba16Float`, so
   the negative components survive — `Rgba8Unorm` would clamp them).
   Depth is also written.

2. *Composite pass* — a full-screen triangle (no vertex buffer; vertex
   indices 0/1/2 map to the NDC corners `(-1,-1)`, `(3,-1)`, `(-1,3)`).
   The fragment shader samples the normal G-buffer and depth at the current
   pixel and its 8 neighbors via `textureLoad`. A pixel is declared an
   *edge* when either:
   - The angle between the center normal and any neighbor exceeds a small
     threshold (crease detection), or
   - The depth differs from any neighbor by more than a threshold
     (silhouette / contour detection).

   Otherwise the pixel gets a flat, brightly lit white. The result: black
   edges on white faces.

Both thresholds are tunable constants near the bottom of `src/renderer.rs`:

```wgsl
const NORMAL_THRESH: f32 = 0.05;   // ≈ creases ≥ 18°
const DEPTH_THRESH:  f32 = 0.0008; // tighter → catches finer silhouettes
```

### Camera angles

Defined in `src/camera.rs` as a `View` enum with 14 variants:

- 6 cardinal (cube face centers): `Front`, `Back`, `Left`, `Right`, `Top`, `Bottom`
- 8 isometric (cube vertices):
  `IsoTopFrontRight`, `IsoTopFrontLeft`, `IsoTopBackRight`, `IsoTopBackLeft`,
  `IsoBottomFrontRight`, `IsoBottomFrontLeft`, `IsoBottomBackRight`, `IsoBottomBackLeft`

Each provides an eye-direction vector — the cardinals point along ±X/±Y/±Z,
the isometrics along the eight `(±1, ±1, ±1)/√3` corner diagonals. The `up`
vector is `+Y` for everything except `Top`/`Bottom`, which use `±Z` so
`look_at_rh` doesn't degenerate when eye and up are colinear. All cameras
look at the origin; only distance and direction differ between views.

Distances are configured in `src/main.rs`:

```rust
const VARIANTS: &[(f32, RenderMode, &str)] = &[
    (2.5, RenderMode::Plain, ""),
    (4.5, RenderMode::Plain, "_far"),
    (2.5, RenderMode::Edges, "_edges"),
    (4.5, RenderMode::Edges, "_far_edges"),
];
```

Smaller distance → larger model on screen. Because the model is normalized
to a unit-radius region, distance is in those normalized units (not mm).

### Buffer-mapping (GPU → CPU readback)

`copy_texture_to_buffer` requires `bytes_per_row` aligned to
`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes). For widths whose natural
row stride (`width × 4`) is already a multiple of 256 — including the
default 1024 — no padding is needed. For other widths, the readback buffer
has padded rows and the per-row padding is stripped while copying into the
tight RGBA buffer the PNG encoder expects.

Mapping is asynchronous:

```
slice.map_async(MapMode::Read, callback)
device.poll(Maintain::Wait)         // blocks main thread until ready
slice.get_mapped_range()            // returns &[u8] aliasing driver memory
... copy out ...
buffer.unmap()                      // release driver memory
```

The callback fires on the wgpu device thread; we forward its `Result` to
the main thread via a `std::sync::mpsc` one-shot channel.

### Parallelism model

| Stage          | Threading                                        |
|----------------|--------------------------------------------------|
| GPU rendering  | Serial on the main thread (single wgpu queue)    |
| GPU readback   | Serial (single 256-byte-aligned staging buffer)  |
| PNG encoding   | Parallel, N worker threads                       |
| File I/O       | Parallel (each worker writes its own file)       |

The GPU stays serial deliberately — adding multi-buffered submission would
add real complexity for marginal gain at this scale. Encoding parallelism is
where the win lives: PNG deflate is more expensive than rendering 10k
triangles to a 1024² texture on modern GPUs.

A bounded `crossbeam-channel` (depth `2 × encoders`) provides backpressure:
the GPU producer can race ahead by at most that many frames before blocking
on `send`, capping in-flight RGBA memory.

## Performance

On Apple Silicon (M-class, default 1024×1024, 10,900-triangle test mesh):

| Configuration                      | 56 images |
|------------------------------------|-----------|
| Cold start (first run, PSO compile)| ~700ms    |
| Warm, 8 encoder threads (default)  | **~84ms** |

Per-image cost in the warm parallel case is about 1.5ms.

## Customizing

Common tweaks, by file:

- **`src/main.rs`** — `VARIANTS` constant. Add or remove distance/mode
  combinations, change suffix naming.
- **`src/camera.rs`** — `View` enum. Add custom camera angles, adjust the
  default FOV (in `view_projection_matrix`), or change the up-vector logic.
- **`src/renderer.rs`** —
  - `NORMAL_THRESH`, `DEPTH_THRESH` in the composite shader to make edge
    detection more or less aggressive.
  - `light_dir` constant in the WGSL shaders to relight.
  - Background color: the `Color { r: 0.07, g: 0.07, b: 0.09, a: 1.0 }`
    clear value (search for it in `render_to_pixels`).
  - Cull mode: switch `Some(Face::Back)` to `None` if your STL has
    inconsistent winding and shows holes.
- **`Cargo.toml`** — wgpu/glam/image versions are pinned conservatively;
  bump as needed.

## Project layout

```
src/
├── main.rs        CLI entry, encoder pool, PNG output
├── parser.rs      STL load → Vertex/index buffers + AABB
├── camera.rs      View enum + view-projection matrix builder
└── renderer.rs    wgpu setup, two pipelines, render passes,
                   GPU→CPU readback
```

About 700 lines of Rust + WGSL total. The code is heavily commented — start
in `renderer.rs::Renderer::new` to follow the wgpu setup linearly.

## Dependencies

- `wgpu` — graphics API abstraction
- `glam` — `Mat4`/`Vec3` math (with `bytemuck` interop for uniform upload)
- `stl_io` — STL parsing
- `image` — PNG encoding
- `clap` — CLI parsing
- `crossbeam-channel` — bounded MPMC for the encoder pool
- `pollster`, `bytemuck`, `anyhow` — async-driver, POD casts, error type

## Platform support

- macOS (Apple Silicon and Intel) — Metal backend
- Linux — Vulkan backend (requires Vulkan-capable GPU + drivers)
- Windows — DX12 backend

The same source produces the same images on all three. wgpu's NDC depth
range is `[0, 1]` (Vulkan/DX/Metal style), so the math in `camera.rs` uses
`Mat4::perspective_rh` directly without GL-style depth remapping.
