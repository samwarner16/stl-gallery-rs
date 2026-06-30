---
name: stl-gallery
description: Render an STL mesh to a 56-image PNG gallery (14 camera angles × 2 distances × 2 styles — shaded and edge-detected). Automatically bootstraps the renderer from GitHub if missing on a new machine.
---

# stl-gallery — 56-view STL preview gallery

This skill renders any `.stl` file into a professional multi-view gallery.

## Bootstrap (for new machines)

The renderer lives in a separate repo. If the binary is missing, Grok should help the user install it:

1. Ensure the project is present:
   ```bash
   mkdir -p ~/projects
   if [ ! -d ~/projects/stl-gallery-rs ]; then
     git clone https://github.com/samwarner16/stl-gallery-rs.git ~/projects/stl-gallery-rs
   fi
   cd ~/projects/stl-gallery-rs
   ```

2. Build the release binary (first time only):
   ```bash
   cargo build --release
   ```

3. The binary will be at: `~/projects/stl-gallery-rs/target/release/stl-gallery-rs`

Add this to your PATH or use the full path.

You can also add a symlink:
```bash
ln -s ~/projects/stl-gallery-rs/target/release/stl-gallery-rs ~/.local/bin/stl-gallery-rs
```

## Invocation

**Preferred resolution:** 2048×2048 (user default)

**Standard command:**
```bash
~/projects/stl-gallery-rs/target/release/stl-gallery-rs \
  -i /path/to/model.stl \
  -o gallery_modelname \
  --width 2048 --height 2048
```

Or with the symlink:
```bash
stl-gallery-rs -i model.stl -o gallery_foo --width 2048 --height 2048
```

### Output

Produces 56 PNG files in the output directory:
- 14 angles: front, back, left, right, top, bottom, iso_top_front_right, ... (all 8 iso variants)
- For each: `{angle}.png` (shaded near), `{angle}_far.png`, `{angle}_edges.png`, `{angle}_far_edges.png`

## Common usage patterns

- For a single STL in current dir: run with `-o gallery`
- For the enclosures project: run on `*_tub.stl` and `*_top_lid.stl` (preview is optional)
- Multiple models: run once per file with descriptive `-o` dirs (e.g. `gallery_Compact-Nano`)

## Tips

- The renderer auto-centers and scales the model.
- Use `--width 1024` for faster previews.
- After generation, you can use image tools or move the gallery into the model's folder.

## Source

The full renderer source and binaries are at:
https://github.com/samwarner16/stl-gallery-rs

Clone anywhere and rebuild on new machines.
