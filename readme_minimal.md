# stl-gallery-rs

Headless command-line tool that renders an STL mesh to a 56-image PNG
gallery: 14 camera angles (6 cardinal + 8 isometric — one per face center
and one per vertex of an axis-aligned cube) × 2 viewing distances × 2 styles
(shaded and edge-detected). Cross-platform (Metal on macOS, Vulkan on Linux,
DX12 on Windows). Both ASCII and binary STL accepted; the model is
auto-centered and scaled to fit.

## Build

```sh
cargo build --release
```

## Run

```sh
./target/release/stl-gallery-rs -i path/to/model.stl -o gallery
```

Options:

```
  -i, --input <FILE>     Input STL file (binary or ASCII)        [required]
  -o, --output <DIR>     Output directory                        [default: gallery]
      --width <PIXELS>   Render width                            [default: 1024]
      --height <PIXELS>  Render height                           [default: 1024]
      --encoders <N>     PNG-encoder worker threads              [default: 2..=8 cores]
```

## Output

56 PNGs written to the output directory, one per (angle, distance, style):

```
{front, back, left, right, top, bottom,
 iso_top_front_right,    iso_top_front_left,
 iso_top_back_right,     iso_top_back_left,
 iso_bottom_front_right, iso_bottom_front_left,
 iso_bottom_back_right,  iso_bottom_back_left}
   ×  {"", "_far"}             # near vs. stepped-back framing
   ×  {"", "_edges"}           # shaded vs. black-edges-on-white
   .png
```

Examples: `front.png`, `front_far.png`, `iso_top_front_right_edges.png`,
`iso_bottom_back_left_far_edges.png`, …
