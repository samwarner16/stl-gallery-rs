use anyhow::Result;
use clap::Parser;
use crossbeam_channel::bounded;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

mod camera;
mod parser;
mod renderer;

#[derive(Parser, Debug)]
#[command(name = "stl-gallery-rs", version, about = "Headless STL → PNG gallery generator")]
struct Args {
    /// Input STL file (binary or ASCII).
    #[arg(short, long)]
    input: PathBuf,

    /// Output directory for PNGs.
    #[arg(short, long, default_value = "gallery")]
    output: PathBuf,

    /// Render width in pixels.
    #[arg(long, default_value_t = 1024)]
    width: u32,

    /// Render height in pixels.
    #[arg(long, default_value_t = 1024)]
    height: u32,

    /// Number of PNG-encoder worker threads. Default: clamp(cores, 2, 8).
    #[arg(long)]
    encoders: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    std::fs::create_dir_all(&args.output)?;

    let mesh = parser::load_stl(&args.input)?;
    println!(
        "Loaded {} triangles, bounds min={:?} max={:?}",
        mesh.indices.len() / 3,
        mesh.bounds.min,
        mesh.bounds.max
    );

    let mut renderer =
        pollster::block_on(renderer::Renderer::new(args.width, args.height, &mesh))?;

    use renderer::RenderMode;
    // (distance, mode, filename suffix). 4 variants × 10 angles = 40 PNGs.
    const VARIANTS: &[(f32, RenderMode, &str)] = &[
        (2.5, RenderMode::Plain, ""),
        (4.5, RenderMode::Plain, "_far"),
        (2.5, RenderMode::Edges, "_edges"),
        (4.5, RenderMode::Edges, "_far_edges"),
    ];

    let aspect = args.width as f32 / args.height as f32;
    let n_workers = args.encoders.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(2, 8)
    });

    // Bounded channel acts as backpressure: the GPU producer can race ahead
    // by at most `2 * n_workers` frames before blocking on send. Caps memory
    // at ~(workers × 2 × width × height × 4) bytes of in-flight RGBA.
    let (tx, rx) = bounded::<(Vec<u8>, PathBuf)>(n_workers * 2);

    let started = Instant::now();
    let total = camera::View::all().len() * VARIANTS.len();

    thread::scope(|s| -> Result<()> {
        // ── Encoder pool ─────────────────────────────────────────────
        // Each worker pulls (pixels, path) tuples and runs PNG deflate
        // independently. Walltime ≈ max(GPU loop, encode loop / N).
        let width = args.width;
        let height = args.height;
        let mut workers = Vec::with_capacity(n_workers);
        for _ in 0..n_workers {
            let rx = rx.clone();
            workers.push(s.spawn(move || -> Result<()> {
                for (pixels, path) in rx {
                    encode_png(&pixels, width, height, &path)?;
                    println!("✔ {}", path.display());
                }
                Ok(())
            }));
        }
        drop(rx); // main thread doesn't read; workers hold the only refs.

        // ── GPU producer (this thread) ───────────────────────────────
        for view in camera::View::all() {
            for &(distance, mode, suffix) in VARIANTS {
                let path = args.output.join(format!("{}{}.png", view.name(), suffix));
                let view_proj = camera::view_projection_matrix(*view, aspect, distance);
                let pixels = pollster::block_on(renderer.render_to_pixels(view_proj, mode))?;
                tx.send((pixels, path)).expect("encoder pool died");
            }
        }
        drop(tx); // signal workers no more work is coming.

        // Surface any worker error.
        for w in workers {
            w.join().expect("worker panic")?;
        }
        Ok(())
    })?;

    let elapsed = started.elapsed();
    println!(
        "Rendered {} images in {:.2?} ({:.1} ms/img, {} encoders)",
        total,
        elapsed,
        elapsed.as_secs_f64() * 1000.0 / total as f64,
        n_workers,
    );

    Ok(())
}

/// Encode a tightly packed RGBA8 buffer to a PNG file.
///
/// `CompressionType::Fast` (deflate level 1) trades a few percent of file
/// size for substantially faster encoding. `FilterType::Adaptive` picks the
/// best PNG row filter per scanline — critical for our shaded renders, where
/// `NoFilter` would inflate output by ~25× on smooth gradients.
fn encode_png(pixels: &[u8], width: u32, height: u32, path: &Path) -> Result<()> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;

    let file = BufWriter::new(File::create(path)?);
    let encoder = PngEncoder::new_with_quality(file, CompressionType::Fast, FilterType::Adaptive);
    encoder.write_image(pixels, width, height, image::ExtendedColorType::Rgba8)?;
    Ok(())
}
