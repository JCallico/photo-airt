# Photo·AIrt

Photo·AIrt is a native Rust desktop studio for turning your photographs into
art. It combines hand-written image algorithms with AI, using the
**Claude Code** and **Codex** CLIs you are already logged into. There are no
API keys: each AI feature appears only when its CLI is found on `PATH`.

![Photo·AIrt studio: style gallery, split before/after compare, atelier controls](docs/screenshots/studio-split.jpg)

## Getting started

You need a Rust toolchain (pinned in `mise.toml`, or any recent stable Rust).
The AI features also need either or both of
[Claude Code](https://claude.com/claude-code) and
[Codex CLI](https://github.com/openai/codex), installed and logged in.

```bash
git clone https://github.com/JCallico/photo-airt.git
cd photo-airt
cargo run --release -- path/to/photo.jpg   # or omit the path and drop a photo on the window
```

On Linux, the native file dialog uses the XDG desktop portal. HEIC files are
decoded through ImageMagick, libvips or `heif-convert` when one of them is
installed.

## Fourteen algorithmic styles

Every style is written from scratch in Rust and runs on all cores with rayon.
The cards in the style gallery show live previews of *your* photo. Selecting
one renders it at working resolution in a second or two. A paint-sweep
animation reveals each new result.

![The same photo in twelve of the fourteen styles](docs/screenshots/styles-grid.jpg)

| Style | Technique |
|---|---|
| Oil on Canvas | Anisotropic Kuwahara filter with polynomial sector weights, structure-tensor flow, impasto relighting |
| Impressionist Strokes | Hertzmann multi-layer curved brush strokes, bristle texture, height-map relief (painted in parallel bands, stroke order preserved) |
| Watercolour | Abstraction, wet-in-wet bleeding, Bousseau pigment-density model, granulation, XDoG underdrawing, deckled border and splatter |
| Starry Flow | Line integral convolution of colour-jittered stroke noise along a smoothed tangent field |
| Pointillism | Jittered dot placement, divisionist hue splitting, a detail pass |
| Graphite Sketch, Ink & Wash, Cel Animation | Colour dodge with LIC hatching, XDoG, soft value quantisation |
| Stained Glass, Low Poly, Pop Art Quad | Jump-flood Voronoi with chamfer-distance lead lines, Delaunay facets, posterised palettes |
| CMYK Halftone, Risograph, Pixel Art | Rotated AM screens, least-squares two-ink separation, k-means++ palette with Bayer dithering |

Each style's parameters appear as sliders in the **Atelier** panel. Every
artwork, including AI results, also gets a non-destructive **Finish** layer:
exposure, contrast, saturation, warmth, vignette, grain, canvas weave and
glow.

## AI studio: Art Director and Master Painter

AI work is split into two roles. Each role can be assigned to either CLI and
gets its own model.

<img src="docs/screenshots/roles.png" alt="Roles card: Art Director and Master Painter, each with a CLI and a model" width="340" align="right">

| Role | Claude | Codex |
|---|---|---|
| **Art Director**: art direction, briefs, vector art, wall labels | ✓ | ✓ (vision through `--image`, read-only sandbox) |
| **Master Painter**: repaints the whole photo | ✓ as an **SVG painting** (Claude Code has no raster image model) | ✓ raster, via its `image_generation` tool |

Roles are resolved from what is installed when the app starts. The
**Rescan** button runs detection again. Detection covers:

* whether each CLI is on `PATH`;
* whether Codex has `image_generation`;
* which models each CLI offers. Codex's list comes from
  `~/.codex/models_cache.json` and its default from `~/.codex/config.toml`.
  Claude gets the aliases `opus`, `sonnet`, `haiku` and `fable`, plus its
  default from `~/.claude/settings.json`.

Saved choices are kept when they are still valid. Otherwise the app falls
back to Claude as Art Director and Codex as Master Painter, whichever exists.

If a model fails with an account or model error, the picker flags it with ⚠
and the reason. An example is "Fable 5.1 requires usage credits". The flag
clears the next time that model succeeds. Role choices and model health are
stored in `~/.config/photo-airt/prefs.json`.

<br clear="right">

What each AI feature does:

* **Art Director:** reads the photo and returns a title, a palette, three
  tuned recipes for the algorithms (one click to apply), and a brief for the
  Master Painter.
* **Master Painter:** repaints from ten presets or your own brief. In *Duet*
  mode, the Art Director first writes a photo-specific prompt for the Master
  Painter.
* **Vector Reinterpretation:** the Art Director hand-writes an SVG (Matisse
  cut-outs, Cubist, Bauhaus, woodblock and others), which is rasterised with
  resvg.
* **Wall labels:** museum placards for the Gallery view.

![AI results: the original photo, a Codex watercolour from a Claude brief, a Claude Matisse cut-out and a Claude SVG painting](docs/screenshots/ai-results.jpg)

AI results are saved to `~/Pictures/Photo-AIrt`. Job scratch files go to
`~/.cache/photo-airt/jobs`. A live studio log streams each CLI's progress,
and running jobs appear in the filmstrip. A Codex repaint usually takes 1–2
minutes, and a Claude SVG takes 1–3.

## Gallery view

The Gallery view hangs the selected artwork in a gilded frame on a lit wall.
The wall label comes from the Art Director, or falls back to the technique.

![Gallery view with a framed artwork and its wall label, next to the AI studio](docs/screenshots/gallery-ai-studio.jpg)

## Using the studio

Views switch with keys `1`–`4`:

* **Split:** a draggable before/after compare with zoom (scroll) and pan
  (drag).
* **Side by side.**
* **Single.**
* **Gallery.**

The filmstrip holds the collection. A new render replaces the current
*draft*. Keep a draft with `K` and it becomes a permanent piece. **Export**
(`Ctrl+S`) re-renders algorithmic pieces at the photo's full resolution and
saves them to `~/Pictures/Photo-AIrt`.

| Key | Action |
|---|---|
| `Space` (hold) | Show the original |
| `1`–`4` | Switch view |
| `←` / `→` | Browse the collection |
| `K` / `R` | Keep the draft / reroll the seed |
| `A` | Switch between Algorithms and AI Studio |
| `Ctrl+O` / `Ctrl+S` | Open / export |

## Command line

```bash
photo-airt --render <style-id|all> in.jpg out_dir [long-side]   # batch-render styles
PHOTO_AIRT_DIRECTOR=codex:gpt-5.6-luna PHOTO_AIRT_PAINTER=claude:sonnet \
  photo-airt --ai <director|vector|repaint|duet|placard> in.jpg [out.png]   # run one AI job
```

## Development

```bash
cargo test --release
cargo clippy --release --all-targets -- -D warnings
cargo fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo deny check   # advisories, licenses, bans, sources (deny.toml)
```

CI runs these checks on every pull request:

- lint (rustfmt, clippy, rustdoc and whitespace);
- build and tests on Linux, macOS and Windows;
- the `cargo-deny` policy;
- GitHub dependency review.

The `cargo-deny` policy also runs weekly, so new RustSec advisories surface
even when the code hasn't changed.

Changes to `main` go through pull requests, and the same checks run in CI.
Contributor and agent guidelines are in [AGENTS.md](AGENTS.md). To report a
security issue, follow [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE) © 2026 Javier Callico. The bundled Inter and Playfair Display
fonts are licensed under the SIL Open Font License 1.1 (`assets/fonts/`).
