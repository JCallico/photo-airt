# Writing Photo·AIrt plug-ins

A plug-in adds a painting style to Photo·AIrt. It appears in the style gallery
next to the 14 built-in styles, with live previews, sliders in the Atelier
panel, the finish layer, export and Art Director recipes, exactly like a
built-in style.

A plug-in is a folder with two parts:

- **`plugin.toml`** describes it: its name, gallery group, description and
  settings, and the command that runs it.
- **A program, in any language**, that renders. For each render it reads an
  input image and a request file, and writes an output image.

There's no library to link against and no binary protocol. If you can read and
write a PNG and print a line of JSON, you can write a plug-in.

This guide covers:

1. [Quick start](#quick-start)
2. [Where plug-ins live](#where-plug-ins-live)
3. [The manifest: `plugin.toml`](#the-manifest-plugintoml)
4. [`render`](#render)
5. [Requirements checklist](#requirements-checklist)
6. [Testing with `--check-plugin`](#testing-with---check-plugin)
7. [Built-in styles are plug-ins too](#built-in-styles-are-plug-ins-too)
8. [Approval and security](#approval-and-security)
9. [Compatibility](#compatibility)
10. [Wrapping existing tools](#wrapping-existing-tools)

---

## Quick start

The repository ships two complete examples, each using only its language's
standard library:

| Example | Language | Shows |
|---|---|---|
| [`retro-print`](../examples/plugins/retro-print) | Python | All three setting types; lookup-table processing |
| [`copperplate`](../examples/plugins/copperplate) | JavaScript (Node.js) | A new algorithm (banknote-style line engraving) that uses `scale` for size-like settings and `seed` for randomness |

To try them in the app straight from a checkout, point the app at the
examples folder and approve them in the Plug-ins panel:

```bash
PHOTO_AIRT_PLUGINS=examples/plugins cargo run --release -- path/to/photo.jpg
```

```bash
# 1. Check it against the contract (doesn't install anything).
photo-airt --check-plugin examples/plugins/retro-print

# 2. Install it: copy the folder into your plug-ins folder.
#    (Plug-ins panel → "Open my plug-ins folder" shows where that is.)
cp -r examples/plugins/retro-print ~/.local/share/photo-airt/plugins/

# 3. In the app: Plug-ins → Rescan → Approve "Retro Print".
#    It now appears in the gallery under "Plug-in examples".
```

To start your own plug-in, copy one of the example folders, change the `id`,
`name` and settings in its `plugin.toml`, and replace the effect.

## Where plug-ins live

Each plug-in is a **folder** containing a `plugin.toml` and whatever else it
needs (scripts, executables, data files). Photo·AIrt looks in these locations,
highest precedence first:

| Location | Path | Use it for |
|---|---|---|
| `PHOTO_AIRT_PLUGINS` | Folders listed in this environment variable (`:`-separated on Linux and macOS, `;`-separated on Windows) | Development, CI and custom setups |
| Your plug-ins folder | Linux: `~/.local/share/photo-airt/plugins/` (or `$XDG_DATA_HOME/photo-airt/plugins/`) · macOS: `~/Library/Application Support/photo-airt/plugins/` · Windows: `%APPDATA%\photo-airt\plugins\` | Normal installs; always writable, per user, survives app upgrades |
| Next to the app | `<folder containing the photo-airt executable>/plugins/` | Portable setups: copy the app's folder and its plug-ins come along |

`photo-airt --plugins` prints the locations and what was found in each. The
Plug-ins panel in the app shows the same, and can create and open your
plug-ins folder.

Rules:

- **Same id in two places:** the plug-in in the highest-precedence location
  wins. The others are listed as not used (shadowed).
- **Clash with a built-in style:** a plug-in can't reuse a built-in style's
  id (for example `oil`). It's reported as an error.

## The manifest: `plugin.toml`

`plugin.toml` is the plug-in's complete description. Photo·AIrt reads it
**without running anything**, so the Plug-ins panel can show what the plug-in
is, who made it and what it does before the user approves it. The gallery,
the Atelier panel's controls, the **Defaults** button and the Art Director's
catalogue all come from it.

```toml
protocol = 1
id = "retro-print"
name = "Retro Print"
family = "Plug-in examples"
blurb = "Posterised colour printed with a vintage ink."
technique = "Per-channel posterisation and ink tone curves…"
version = "1.0.0"
author = "Photo·AIrt"
licence = "MIT"
timeout_seconds = 60
run = ["python3", "retro_print.py"]
run_windows = ["python", "retro_print.py"]

[[settings]]
key = "levels"
label = "Tone levels"
type = "number"
min = 2
max = 12
step = 1
default = 5

[[settings]]
key = "invert"
label = "Invert (negative)"
type = "toggle"
default = false

[[settings]]
key = "ink"
label = "Ink"
type = "choice"
default = "sepia"
options = [
  { value = "natural", label = "Natural colour" },
  { value = "sepia", label = "Sepia" },
]
```

| Field | Required | Meaning |
|---|---|---|
| `protocol` | yes | Contract versions you speak: a number or a list. This guide describes version `1`. |
| `id` | yes | Stable, unique id: lowercase letters, digits, `-` and `_`. **Never change it**: saved recipes and the Art Director refer to it. |
| `name` | yes | Display name. |
| `run` | yes | Command and arguments that render. See below. |
| `run_windows`, `run_macos`, `run_linux` | no | Replace `run` on that operating system. |
| `family` | no | Gallery group. Use an existing one ("Painterly", "Drawing & Ink", "Geometric", "Print & Pixel") or your own. Defaults to "Plug-ins". |
| `blurb` | no | One line shown in the Plug-ins panel, on hover and to the Art Director. |
| `technique` | no | Shown under "How it works". |
| `version`, `author`, `licence` | no | Shown in the Plug-ins panel. |
| `timeout_seconds` | no | Longest a render may take (default 120, maximum 600). |
| `settings` | no | The controls in the Atelier panel, as `[[settings]]` tables. |

About `run`:

- The first element is either a program on your `PATH` (`python3`, `node`,
  `magick`…) or a file inside the plug-in folder (`my-plugin`,
  `bin/my-plugin.exe`).
- The command always runs with the **plug-in folder as its working
  directory**, so relative paths in the arguments work.

Unknown keys are rejected, so typos show up as errors. An invalid manifest is
reported in the Plug-ins panel and by `--check-plugin`.

### Settings

Every setting has a `key` (stable, unique within the plug-in), a `label`, an
optional `help` tooltip, a `type` and a `default`. There are three types:

| `type` | Extra fields | Control | Value in requests |
|---|---|---|---|
| `number` | `min`, `max`, `step` (`step` ≥ 1 gives an integer slider; 0 means continuous) | Slider | JSON number |
| `toggle` | none | Checkbox | `true` / `false` |
| `choice` | `options`: a list of `{ value = …, label = … }` | Drop-down | The chosen option's `value` string |

The `default` must be valid: within range, a boolean, or one of the options.

## `render`

For every render (previews, the working image, exports), Photo·AIrt starts
**a new process** and nothing persists between renders. It:

1. creates a private, temporary job folder;
2. writes `input.png` (8-bit RGB) and `request.json` into it;
3. runs `<run command> render <job folder>/request.json`.

`request.json`:

```json
{
  "protocol": 1,
  "plugin_id": "retro-print",
  "input": "/…/plugin-jobs/…/input.png",
  "output": "/…/plugin-jobs/…/output.png",
  "width": 2048,
  "height": 1536,
  "params": { "levels": 5, "invert": false, "ink": "sepia" },
  "seed": 7,
  "scale": 1.28
}
```

| Field | Meaning |
|---|---|
| `input` | The photo to paint, as a PNG, at the size being rendered. |
| `output` | Where you must write your result: a PNG of **exactly `width` × `height`**. |
| `params` | The current value of every setting declared in `plugin.toml`, by key, already checked against its range, options and type. |
| `seed` | Drives any randomness (an integer below 2^53). **The same seed must give the same picture**: previews, exports and recipes rely on it, and **Reroll** just changes it. |
| `scale` | This image's long side divided by 1600 px. Multiply size-like settings (brush sizes, cell sizes, line widths) by it so a 420 px thumbnail, the 2048 px working image and a full-resolution export all look alike. |

While working, you **may** print JSON Lines to stdout:

```text
{"progress": 0.4}               progress from 0 to 1 (drives the progress ring)
{"log": "loaded model", "level": "info"}
```

When you're done:

- **Success:** write `output` and exit with code 0.
- **Failure:** print `{"error": "a clear, human-readable reason"}` and exit
  with a non-zero code. The message is shown to the user.

Photo·AIrt then checks that `output` exists, is a valid image and has the
input's size. Anything else counts as a failed render.

Other rules:

- **Cancellation:** when the user moves on (for example by dragging a slider
  again), Photo·AIrt ends your process. You don't need to handle a cancel
  message, but don't leave work behind that relies on a clean shutdown.
- **Timeout:** a render that exceeds `timeout_seconds` is ended and reported
  as failed.
- **Clean-up:** the job folder is deleted after every render, whatever the
  outcome.
- **Environment:** you get a minimal environment: `PATH`, home and temporary
  folders, locale, and `PHOTO_AIRT_PLUGIN_PROTOCOL=1`. Don't expect other
  variables or credentials.
- **stderr:** captured. The last lines are shown when a render fails, so it's
  a good place for debugging output.

The input PNG is written unfiltered with fast compression, so even a
pure-Python decoder reads it quickly. Your output can be any valid 8-bit PNG.

## Requirements checklist

A plug-in works with Photo·AIrt when it meets all of these:

- [ ] `plugin.toml` is valid: it speaks contract version 1, and has a
  stable, unique `id` that doesn't clash with a built-in style, a `name` and
  a working `run` command (plus OS overrides if needed).
- [ ] Every setting has a stable `key` and a valid `default`.
- [ ] `render` writes a PNG of exactly the input size.
- [ ] **Deterministic:** the same input, settings and `seed` always produce
  the same picture.
- [ ] **Resolution independent:** size-like settings are multiplied by
  `scale`.
- [ ] Reports failures with `{"error": …}` and a non-zero exit code.
- [ ] Finishes within `timeout_seconds`. Aim for a few seconds at 2048 px,
  because previews and slider changes re-render often.
- [ ] **Self-contained (D8):** brings everything it needs in its own folder.
  Photo·AIrt provides nothing at run time: no shared libraries, no host
  functions to call back into. Rust plug-ins may compile in the
  `photo-airt-sdk` crate (see
  [Writing a plug-in in Rust](#writing-a-plug-in-in-rust)), which makes the
  built-in image functions part of the plug-in's own executable.
- [ ] Doesn't use the network, and doesn't write files other than `output`,
  temporary files inside the job folder, and its own plug-in folder. This is
  required of plug-in authors; Photo·AIrt can't enforce it.
- [ ] Starts quickly. Every render is a new process, so heavy start-up
  (loading a large model, importing big libraries) is paid on every slider
  change.

Built-in styles, in `plugins/<id>/` of this repository, must also stay
movable, so each one can run as an external plug-in (see
[Built-in styles are plug-ins too](#built-in-styles-are-plug-ins-too)):

- [ ] The folder is named after the style's `id`.
- [ ] `src/lib.rs` takes its description from the folder's own `plugin.toml`
  (`Manifest::embedded(include_str!("../plugin.toml"))`), so there is one
  source of truth.
- [ ] `plugin.toml` runs `bin/<id>` (`bin/<id>.exe` on Windows).
- [ ] `Cargo.toml` builds a binary named `<id>` whose `main` calls
  `photo_airt_sdk::serve` with the crate's `STYLE`.
- [ ] The crate is listed in the app's `Cargo.toml` and in `STYLES` in
  `src/plugins/builtin.rs`.

A unit test checks the folder, description, command and binary rules. A CI step
builds every built-in executable, moves each folder out of the repository
and runs `--check-plugin` on it.

## Testing with `--check-plugin`

```bash
photo-airt --check-plugin path/to/my-plugin
```

This runs your plug-in without installing or approving it. It checks:

- the manifest and the description in it;
- the folder checksum;
- renders at two sizes;
- determinism (the same seed rendered twice must match).

It prints ✓ or ✗ per check and exits non-zero if anything fails, so you can
use it in your own CI.

You can also run the contract by hand: create `input.png` and a
`request.json` like the one above, then run your command with
`render request.json`.

### A reference implementation

Every built-in style can be run through the same contract:

```bash
photo-airt --run-plugin oil render request.json
```

That makes the app itself a reference implementation.

## Built-in styles are plug-ins too

The 14 built-in styles are written exactly like external plug-ins. Each one
has its own folder under [`plugins/`](../plugins) with the same layout:

```text
plugins/oil/
├── plugin.toml      the description and settings; run = ["bin/oil"]
├── Cargo.toml       a crate built on photo-airt-sdk, with a binary named oil
└── src/
    ├── lib.rs       description() (embeds plugin.toml) and render(): the style
    └── main.rs      fn main() { photo_airt_sdk::serve(&photo_airt_oil::STYLE) }
```

There are two differences from an external plug-in:

- **Where the code runs.** Photo·AIrt links each built-in crate into its
  single executable (D3) and calls `render` directly, with no process, job
  folder or PNG files. The `bin/` executable is not needed. The crate embeds
  its `plugin.toml` at compile time, so the description is the same file in
  both cases.
- **Trust.** Built-in styles ship with the app, so they need no approval.

The style code is identical in both cases. To run a built-in style as an
external plug-in:

1. Build its executable into its folder (from the repository root):

   ```bash
   cargo install --locked --path plugins/oil --root plugins/oil
   ```

   This writes `plugins/oil/bin/oil` (`oil.exe` on Windows), which is what
   `plugin.toml` runs.
2. Move the folder to a plug-ins location (see
   [Where plug-ins live](#where-plug-ins-live)), and remove the style from
   `STYLES` in `src/plugins/builtin.rs` and from the app's `Cargo.toml`.
   While a built-in with the same id exists, the Plug-ins panel reports
   the clash instead of loading the plug-in.
3. Approve it in the Plug-ins panel. It renders through the external
   contract, pixel for pixel the same as when it was built in.

To rebuild the moved folder away from the repository, change the
`photo-airt-sdk` path in its `Cargo.toml` to wherever the SDK lives (or to a
`git` dependency on this repository), then run
`cargo install --locked --path . --root .` inside it.

### Writing a plug-in in Rust

The same SDK is the easiest way to write a new plug-in in Rust. Copy a
built-in folder and give the package and the `[[bin]]` your own id. Then
describe the plug-in in `plugin.toml`:

```toml
protocol = 1
id = "duotone"
name = "Duotone"
family = "My styles"
run = ["bin/duotone"]
run_windows = ["bin/duotone.exe"]

[[settings]]
key = "contrast"
label = "Contrast"
type = "number"
min = 0.5
max = 2.0
step = 0.01
default = 1.0
```

and write the effect in `src/lib.rs`:

```rust
use photo_airt_sdk::imaging::*;
use photo_airt_sdk::{Ctx, Description, Manifest, Params, Style};

pub const STYLE: Style = Style { description, render };

pub fn description() -> Description {
    Manifest::embedded(include_str!("../plugin.toml"))
}

pub fn render(src: &Img, p: &Params, ctx: &Ctx) -> Option<Img> {
    let k = p.get("contrast");
    ctx.progress(0.5);
    Some(src.map(|c| {
        let t = ((luma(c) - 0.5) * k + 0.5).clamp(0.0, 1.0);
        mix(hex(0x1d3557), hex(0xf1c27d), t)
    }))
}
```

`photo_airt_sdk::serve` handles the `render` command, reading the request,
the PNG files, progress lines and errors. `photo_airt_sdk::imaging` holds the
raster primitives the built-in styles use: blurs, flow fields, LIC, Kuwahara
and XDoG filters, noise and a seeded `Rng`. They're compiled into your
executable, so the plug-in stays self-contained (D8).

## Approval and security

A plug-in is a program that runs with the user's permissions, so Photo·AIrt
treats it with care (D7):

- **Never run without approval.** Discovery only reads `plugin.toml`. A
  plug-in's program never runs until the user clicks **Approve** in the
  Plug-ins panel, which shows the manifest's name, id, version, author,
  family and blurb first.
- **Approval is pinned to a checksum.** The approval covers a SHA-256 of
  every file in the plug-in folder (hidden files and `__pycache__` are
  ignored). If any file changes, the plug-in shows "Changed since you
  approved it" and must be approved again.
- **Approvals are per user.** They're stored in your configuration folder,
  never inside a plug-ins folder, and apply to one location: the same plug-in
  copied elsewhere needs its own approval.
- **Folder size limit:** plug-in folders over 512 MB are refused, because
  they're hashed in full.

As an author, keep your plug-in folder small and stable, and publish your
source so users can trust what they approve.

## Compatibility

Contract versions are **supported forever** (D10). A plug-in written for
version 1 keeps working in every future Photo·AIrt release.

- New features are added in a backwards-compatible way: new optional fields,
  and new setting types announced in new versions.
- If a version 2 ever exists, list every version you support in `protocol`
  (for example `[1, 2]`), and the app uses the highest one you both speak.

## Wrapping existing tools

Because the contract is "read a PNG, write a PNG", a few lines of script can
turn existing tools into Photo·AIrt styles:

- **ImageMagick:** `magick input.png -paint 4 output.png`.
- **G'MIC:** `gmic input.png fx_painting … -o output.png`.
- **GEGL:** `gegl input.png -o output.png -- gegl:cartoon`.
- **OpenCV:** for example `cv2.stylization` or `cv2.pencilSketch`.

Map the tool's options to `number`, `toggle` and `choice` settings, and
multiply size-like options by `scale`. Users need the tool installed; report
clearly with `{"error": …}` when it's missing. Bridges like these are tracked
as BL-002 in [BACKLOG.md](../BACKLOG.md).
