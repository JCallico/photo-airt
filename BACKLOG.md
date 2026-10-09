# Backlog

Future features and improvements for Photo·AIrt, in rough priority order.
Each item has an ID, a status and enough context for someone to pick it up
cold.

**Statuses:** `Proposed` (idea, not yet designed) · `Design` (needs a design
decision before coding) · `Ready` (designed, can be built) · `In progress` ·
`Done` (move to the changelog or delete).

**How to use this file**

- Add new items at the bottom of the list with the next free `BL-nnn` ID.
- Reference the ID in branch names and pull requests (for example
  `feat/bl-001-plugins`).
- When an item needs a library or an architectural choice, list the options
  under **Open questions** and resolve them with the maintainer before
  building (see "Libraries before code" in [AGENTS.md](AGENTS.md)).
- Honour the standing project rules: no accounts or per-user setup, no API
  keys, and all AI through the logged-in `claude` and `codex` CLIs.

---

## BL-001 · Plug-in system for painting algorithms

**Status:** Ready · **Area:** `src/styles/`, `src/app.rs`, UI, docs

### Summary

Turn the painting algorithms into plug-ins. Every algorithm that ships today
(the fourteen styles: Oil on Canvas, Impressionist Strokes, Watercolour,
Starry Flow, Pointillism, Graphite Sketch, Ink & Wash, Cel Animation, Stained
Glass, Low Poly, Pop Art Quad, CMYK Halftone, Risograph, Pixel Art) is
converted to the same plug-in interface and ships **out of the box**, with
nothing for the user to install. Third parties can then add algorithms
without changing the application. The interface, its requirements and a
how-to guide are documented.

### Motivation

- The application's value grows with the number of looks it can produce, and
  the maintainer cannot write every algorithm.
- Today a style is a `StyleDef` entry in a static table. Adding one means
  editing core files (`styles/mod.rs`, the family enum, the prompt catalogue),
  so contributions are intrusive.
- Making the built-ins use the same interface keeps the plug-in contract
  honest: if our own styles can't be expressed through it, outside authors
  can't either.

### Goals

1. **One interface for everything.** The built-in styles become plug-ins
   through the same loader and registry that external ones use, with no
   special cases in the UI.
2. **Zero setup.** Built-in plug-ins are available on first launch. External
   plug-ins are discovered from a documented folder, with no account, build
   step or configuration for the user.
3. **Everything downstream keeps working** for any plug-in, built-in or
   external:
   - the style gallery with live previews;
   - parameter sliders generated from the plug-in's schema;
   - the finish layer, split compare, gallery view and export;
   - progress and cancellation;
   - AI recipes from the Art Director (the style catalogue given to Claude or
     Codex is built from the loaded plug-ins).
4. **Trusted by consent, contained on failure.** An external plug-in is a
   program the user chooses to run (see decision D1). It never runs without
   the user's explicit approval, and a crashing, hanging or misbehaving
   plug-in cannot take the app down or freeze the UI.
5. **Documented.** An authoring guide with a working example, the full
   requirements list and a validation tool.

### Requirements for the plug-in contract

These are the requirements every plug-in must meet to work with the
application. They are derived from how the built-in styles behave today and
become the documented contract.

**Identity and metadata (reported by the plug-in, D4)**

- A stable, unique `id`. It is referenced by saved recipes, `recents`, AI
  output and the command line, so it must never change between versions.
- Display `name`, `family` (group in the gallery), a one-line `blurb`, a
  `technique` description (shown under "How it works"), `version`, author and
  licence, and the plug-in API version it targets.
- Its **settings**: the controls the app shows in the right-hand Atelier
  panel for this style. Each setting has a stable `key` (used in requests,
  saved recipes and by the Art Director), a label, an optional help text, a
  type and a default. Proposed types for version 1:
  - `number`: minimum, maximum and step. A step of 1 or more gives an integer
    slider. This is what every built-in style uses today.
  - `toggle`: on or off.
  - `choice`: one of a fixed list of named options, for example ink pairs.

  Defaults must be valid, and keys must never be renamed once published. The
  app builds the controls, the defaults, the "Defaults" reset and the Art
  Director's catalogue entirely from this description.

**Rendering behaviour**

- Pure function of input: `(image, params, context) -> image or cancelled`.
  The output has the **same dimensions** as the input and finite pixel values.
- **Deterministic** for a given seed. The app uses the seed for "Reroll", for
  repeatable AI recipes and for identical previews and exports.
- **Resolution independent.** Size-like parameters are authored for a
  reference photo size and scaled by the context, so thumbnails (about 420 px),
  working renders (about 2048 px) and full-resolution exports look alike.
- **Reports progress** (0 to 1) and **honours cancellation** in long loops, so
  dragging a slider never queues stale renders.
- **Bounded time and memory.** It must finish a working-resolution render in
  a reasonable time (target: a few seconds on a modern laptop) and stay within
  a documented memory budget.
- May use multiple threads, but only within the thread budget the host gives
  it.

**Safety and isolation**

- An external plug-in runs as a separate process with the user's permissions,
  so its behaviour is only as trustworthy as its author. The host therefore:
  - never runs a newly discovered plug-in without explicit approval, and
    asks again if the executable changes (trust is pinned to its SHA-256);
  - starts it with a minimal environment and the plug-in's own folder as the
    working directory, and passes no secrets or credentials;
  - applies a per-render timeout and kills the process on cancel or timeout.
- Plug-ins must not access the network or files other than their own folder.
  This is a documented requirement checked by review, not something the host
  can enforce portably.
- A crash, timeout or protocol error in a plug-in is reported as a toast and a
  failed render. Because every render is a fresh process (D2), nothing carries
  over to the next one. It must never crash the app or freeze the UI.
- Input photos are untrusted content. Plug-ins must not assume a size or a
  value range beyond the documented ones.

**Distribution**

- A plug-in is a self-contained folder inside one of the plug-in locations
  (D9): the user's plug-ins folder in the OS's standard data location, the
  portable `plugins` folder next to the executable, or a folder listed in
  `PHOTO_AIRT_PLUGINS`. It
  holds a manifest file (`plugin.toml`) and the executable it names, plus any
  files the executable needs. Separate builds per OS and CPU are the author's
  responsibility.
- Loading is automatic at startup and via a "Rescan" action, and invalid
  plug-ins are reported with a clear reason instead of being skipped silently.
- Built-in plug-ins are embedded in the binary and can't be removed, but can
  be shadowed or disabled.

### Decisions

**D1 · External plug-ins run as separate processes** (decided
2026-10-09). A plug-in is an executable written in any language. It follows a small,
documented command-line contract, the same pattern the app already uses for
the `claude` and `codex` CLIs. We chose this over
WebAssembly, native libraries, embedded scripting and declarative recipes
because any language and toolchain can produce a plug-in.

Consequences we accept:

- No portable sandbox. Safety comes from consent and containment (see
  "Safety and isolation") rather than from isolation.
- Plug-ins ship per-OS/CPU executables, or depend on an interpreter the user
  already has (for example `python3`).
- Images cross the process boundary as files (D5), which costs some encoding
  time per render.

Declarative recipe plug-ins stay a possible later addition. They don't
conflict with D1.

**D2 · One process per render, no sessions** (decided 2026-10-09). Each render
is a self-contained transaction:

1. The host starts the plug-in's executable.
2. The plug-in reads one request (parameters, seed, scale and the input
   image).
3. It reports progress, then produces one result (or one error).
4. The process exits.

Nothing is kept between renders: no long-running background processes, no
state, no handshake to maintain.

Consequences we accept:

- Each render pays the plug-in's start-up cost: negligible for compiled
  plug-ins, roughly 0.1–0.5 s for interpreted ones such as Python with NumPy.
  Authors of slow-starting plug-ins are told this in the guide.
- Plug-ins cannot cache data in memory between renders (for example loaded
  models). If they need to, they must do it themselves on disk in their own
  folder, which is their responsibility.

What we gain:

- Simpler, more predictable behaviour.
- Cancelling is simply ending the process.
- A crash affects only one render.
- No idle processes use memory.

**D3 · A single, self-contained executable** (decided 2026-10-09). Photo·AIrt
ships as one executable with nothing else to install or unpack. The built-in
styles are compiled into it and run in process, and the app works fully
offline out of the box with no plug-ins folder. External plug-ins are a
purely optional add-on. The example plug-ins live in the repository for
authors and CI, and are not part of the release. No feature may require
extra files to work out of the box.

**D4 · Plug-ins describe themselves, including their settings** (decided
2026-10-09). The app gets a plug-in's identity and its configuration settings
(the controls shown in the right-hand Atelier panel) from the plug-in itself,
not from a separate hand-written file that could drift out of date. This is a
second kind of transaction under D2, `describe`:

1. The host starts the plug-in with a `describe` request.
2. The plug-in answers with its metadata and settings schema.
3. The plug-in exits.

Running code just to list it needs care, so:

- A tiny `plugin.toml` names only the executable and a display name. That's
  enough to list the plug-in and ask the user to approve it without running
  anything.
- `describe` runs only after approval. Its answer is cached, keyed by the
  executable's SHA-256 checksum, so it isn't re-run on every launch, and it's
  refreshed when the executable changes or on Rescan.
- Built-in styles answer `describe` from compiled-in data (D3).

**D5 · Images are exchanged as files in a per-render job folder** (decided
2026-10-09). The app does not stream pixels through pipes. For each render
it:

1. creates a fresh, private job folder;
2. writes the input image and a request file there;
3. runs the plug-in with the request file's path;
4. reads the output image the plug-in wrote to the path given in the request.

stdout carries only small JSON Lines messages: progress, log lines and
errors.

Why:

- Plug-ins become trivial to write. Any script around existing tools
  (ImageMagick, OpenCV, G'MIC, GEGL, machine-learning models) can just read
  a file and write a file, with no binary protocol to implement.
- It's easy to debug: inspect the files, or rerun a render by hand.
- It mirrors the existing Codex integration, which already uses per-job
  folders.

Consequences we accept:

- PNG encoding and decoding adds roughly 50–150 ms per render at 2048 px
  (negligible for thumbnails).
- 8-bit colour by default.
- The host must clean up job folders on every outcome: success, error,
  cancel, timeout or crash.

**D6 · Settings types for version 1: `number`, `toggle` and `choice`**
(decided 2026-10-09). These are enough for version 1. New types (for example
`color` or `text`) can be added later as additive protocol changes (D10).

**D7 · Trust is approval plus SHA-256 pinning** (decided 2026-10-09). A
plug-in never runs until the user approves it, and approval is pinned to the
executable's SHA-256 checksum. Any change to the executable requires
approving it again. Signed plug-ins, a curated index and OS-level sandboxing
are out of scope for now; they may become separate backlog items later.

**D8 · Plug-ins are self-contained** (decided 2026-10-09). The contract does
not expose the host's internal primitives (blurs, flow fields, XDoG and so
on) to plug-ins. A plug-in brings everything it needs. This keeps the
contract small and stable, and lets plug-ins use any library they like.

**D9 · Plug-ins are discovered in several folders** (decided 2026-10-09,
extended the same day). The app scans these locations, in this order of
precedence:

1. **Folders listed in `PHOTO_AIRT_PLUGINS`** (optional): an OS path list,
   `:`-separated on Linux and macOS and `;`-separated on Windows. This is for
   development, CI and unusual setups. Nobody needs to set it.
2. **The user's plug-ins folder**, in each OS's conventional per-user data
   location (the platform data directory, as returned by the `dirs` crate the
   app already uses for its settings and cache):
   - Linux: `$XDG_DATA_HOME/photo-airt/plugins/`, by default
     `~/.local/share/photo-airt/plugins/`;
   - macOS: `~/Library/Application Support/photo-airt/plugins/`;
   - Windows: `%APPDATA%\photo-airt\plugins\`.

   It's always writable, it is per user, and it survives replacing or
   upgrading the app.
3. **The portable plug-ins folder:**
   `<folder containing the photo-airt executable>/plugins/`. Copying the app's
   folder copies its plug-ins too, matching D3.

Each location holds one subfolder per plug-in. Rules:

- Missing locations are simply skipped. With none present, the app runs
  normally with only the built-in styles.
- **Same id in several places:** the plug-in from the highest-precedence
  location wins, and the others are listed in the Plug-ins panel as
  "shadowed" so nothing is hidden silently. This lets a user override a
  portable plug-in with their own version without touching the app folder.
- External plug-ins never replace built-in styles by id. A clash is reported
  as an error, so built-in behaviour and saved recipes stay predictable.
- Resolve the executable's real location (following symlinks), so a symlink
  in `PATH` still finds the right portable folder.
- The Plug-ins panel lists every location it scanned, whether it exists, and
  which plug-ins came from where. It also offers to create and open the user
  folder, because OS data folders are often hidden (for example `~/Library` on
  macOS and `%APPDATA%` on Windows).
- Approvals (D7) are per plug-in *file* (location plus checksum), so the same
  plug-in found in two places is approved separately. Approvals and the
  `describe` cache (D4) live in the user's configuration and cache folders,
  never inside a plug-ins folder.

**D10 · Contract versions are supported indefinitely** (decided 2026-10-09).
Once published, a contract version is never dropped: a plug-in written for
version 1 keeps working in every future Photo·AIrt release. Consequences:

- Changes are additive whenever possible, such as new optional fields or new
  setting types. A plug-in announces in `describe` which versions it speaks,
  and the host uses the highest version both sides support.
- A genuinely incompatible change gets a new contract version number, and the
  host keeps implementing all earlier versions alongside it.
- CI keeps example plug-ins for every published version, so a regression
  breaks the build.

### Design

**Architecture**

1. A Rust trait, `StylePlugin` (manifest, parameter schema, `render`), is the
   single contract inside the app. The registry replaces the static `STYLES`
   table, and the UI, previews, export, `--render`, recents and the AI style
   catalogue all read from it.
2. The fourteen built-in styles implement `StylePlugin` directly, in process.
   They stay native Rust for speed and zero setup, and are registered exactly
   like external plug-ins.
3. `ProcessPlugin` implements the same trait by talking to an external
   executable through the protocol, so the rest of the app cannot tell the
   difference.
4. The protocol is proven by shipping example external plug-ins in the
   repository, one in Rust and one in Python. CI runs the validator against
   them.

**Discovery and lifecycle**

- The app reads `plugin.toml` without executing anything, so listing plug-ins
  is always safe. It holds only the command to run and a display name.
  Everything else (id, family, blurb, technique, version, author, licence,
  supported protocol versions and settings) comes from the
  plug-in's `describe` answer (D4), cached by checksum.
- Once the user has approved a plug-in, every render starts its executable
  fresh (D2) with the plug-in's folder as working directory. The request
  states the protocol version, and the plug-in must refuse versions it does
  not support with an `error` and exit.
- At most one render per plug-in runs at a time. A newer request (for example
  from a slider change) kills the older process and starts a new one. The
  app's existing slider debounce keeps this from happening on every pixel of
  movement.
- A per-render timeout from the manifest (with a host maximum) ends runaway
  processes.

**Contract (version 1, sketch)**

The contract is a command line, files in a job folder, and JSON Lines on
stdout. stderr is captured into the studio log for debugging.

*Describe* (D4): `<command> describe`

- The plug-in prints one JSON object to stdout and exits with code 0.
- The object holds: `protocol` (supported versions), `id`, `name`, `family`,
  `blurb`, `technique`, `version`, `author`, `licence`, `settings` (the
  schema; see "Identity and metadata") and an optional `timeout_seconds`.

*Render* (D2, D5): `<command> render <job-folder>/request.json`

The host first creates a fresh job folder, readable and writable only by the
user, under `~/.cache/photo-airt/plugin-jobs/`. In it, it writes:

- `input.png`: the photo, lossless 8-bit RGB, at the size being rendered;
- `request.json`, containing:

  ```json
  {
    "protocol": 1,
    "plugin_id": "my-style",
    "input": "/…/job/input.png",
    "output": "/…/job/output.png",
    "width": 2048,
    "height": 1536,
    "params": { "radius": 6, "texture": true, "inks": "pink-blue" },
    "seed": 7,
    "scale": 1.28
  }
  ```

  - `params` holds the current value of every setting the plug-in declared,
    keyed by setting `key`.
  - `seed` drives any randomness. The same seed must give the same picture.
  - `scale` is the size of this image relative to the reference size (a
    1600 px long side), for scaling size-like settings.

While working, the plug-in may print JSON Lines to stdout:

- `{"progress": 0.4}`, with values from 0 to 1;
- `{"log": "…", "level": "info"}`;
- on failure, `{"error": "human-readable reason"}`, then exit non-zero.

On success, the plug-in writes `output` as a PNG and exits with code 0.

The host then checks that `output` exists, decodes as an image (through the
same safe decoder used for photos) and has the same width and height as the
input. Anything else is a failed render. The host deletes the job folder on
every outcome.

Other rules:

- Cancel means the host ends the process and deletes the job folder. There is
  no cancel message.
- The plug-in must refuse protocol versions it does not support with an
  `error` and exit.
- It may only write its output file, temporary files inside the job folder,
  and files inside its own plug-in folder.
- Possible later extensions, declared by the plug-in in `describe`:
  - 16-bit or float images (for example TIFF or PFM) for plug-ins that need
    more precision;
  - passing the original full-resolution file at export.

**SDKs (optional, thin)**

- Small helper libraries (Rust crate, Python module) handle reading
  `request.json`, printing progress lines and checking the output, so a
  plug-in author only writes `render(image, params, ctx)`. They are optional:
  a ten-line shell script around an existing tool is a valid plug-in.

**Built-ins through the same protocol (optional)**

- The executable can run any built-in style through the same contract, for
  example `photo-airt --run-plugin oil render request.json` and
  `photo-airt --run-plugin oil describe`. This tests the contract end to end
  with no extra files, and gives authors a reference implementation (D3).

### Comparison with existing plug-in ecosystems

How our contract compares with the tools people already use, and whether
their plug-ins can be reused.

| | Photo·AIrt (this design) | ImageMagick | OpenCV | GIMP |
|---|---|---|---|---|
| **What a "plug-in" is** | Any executable following a command-line contract | Mostly built-in operators (`-paint`, `-sketch`, …). True extensions are C modules (coders, `-process` filters) compiled against MagickCore | A library, not a plug-in host. Its "plugins" are internal back-ends (video I/O, GUI), not effects | Separate executables (C, Python 3, Scheme) that talk to GIMP over its own wire protocol using `libgimp` |
| **Isolation** | Separate process per render | In-process (modules); separate process when called as a CLI | In-process (it's a library) | Separate process, but long-lived and calling back into GIMP |
| **Describes its settings** | Yes: `describe` returns a settings schema (D4) | No machine-readable schema | No | Yes: plug-ins register procedures with typed arguments in GIMP's procedure database (closest to our D4) |
| **Data exchange** | Files plus JSON Lines | Files, or in-memory inside the library | In-memory arrays | Shared memory and tiles via `libgimp`; images live inside GIMP |
| **Runs without the host app** | Yes, any plug-in can be run by hand on files | Yes (CLI) | Yes (it's a library) | No: a GIMP plug-in needs a running GIMP |
| **Language** | Any | C for modules; any language via the CLI | C++, Python and others | C, Python 3, Scheme (Script-Fu) |

**What this means for reuse:**

- **ImageMagick and OpenCV can be leveraged easily.** Neither has a plug-in
  catalogue to import, but because our contract is "read a file, write a
  file", a plug-in can be a short script that calls `magick` or uses OpenCV
  (for example `cv2.stylization`, `cv2.pencilSketch`, `cv2.edgePreservingFilter`)
  and declares its settings in `describe`.
- **GIMP's own plug-ins can't run without GIMP**, because they depend on
  `libgimp` and GIMP's procedure database. Running GIMP headless as a bridge
  is possible but heavy and fragile, so it isn't recommended.

  GIMP's effects largely come from two engines that *do* have command-line
  tools:
  - **GEGL** (`gegl input.png -o output.png -- gegl:cartoon …`), GIMP's
    image-processing engine, with dozens of filters such as `gegl:cartoon`,
    `gegl:oilify`, `gegl:photocopy`;
  - **G'MIC** (`gmic input.png <filter> -o output.png`), the very popular
    filter collection with hundreds of artistic filters, available both as
    a GIMP plug-in and as a standalone CLI.

  Bridging to these reaches most of what GIMP users think of as "GIMP
  filters".
- Our design is conceptually closest to GIMP's (out-of-process plug-ins with
  self-described, typed settings), but deliberately simpler: one transaction
  per process, files instead of a binary wire protocol, and no callbacks into
  the host.

Bridges are optional follow-up work, tracked as **BL-002**. They don't change
the BL-001 contract: a bridge is just an ordinary external plug-in.

### Scope of work

1. Define the plug-in interface and manifest format, and version the plug-in
   API.
2. Introduce a plug-in registry replacing the static `STYLES` table. The UI,
   previews, export, command line (`--render`), recents and the AI style
   catalogue all read from it.
3. Convert the fourteen built-in styles into built-in plug-ins, with their ids,
   parameter keys and defaults unchanged, so existing recipes and saved data
   keep working.
4. Plug-in discovery: a documented per-user folder, a rescan action, and a
   "Plug-ins" panel showing what is loaded, its version and its load errors.
5. The external-process runtime (`ProcessPlugin`):
   - the contract implementation (`describe`, `render`) and its version check;
   - one process per render: create the job folder, spawn, read progress and
     the result, enforce the exit status;
   - timeouts and cancellation;
   - capturing stderr into the studio log;
   - the trust prompt pinned to a SHA-256 checksum.
6. An authoring guide (`docs/plugins.md`) with:
   - the full requirements list above;
   - a worked example plug-in;
   - the manifest and parameter schema reference;
   - the host primitives available;
   - testing and debugging advice;
   - how to submit a plug-in to the project.
7. A validation command, for example `photo-airt --check-plugin <path>`. It
   verifies:
   - the manifest;
   - that defaults are in range;
   - same-size, finite output;
   - determinism across two runs;
   - resolution independence at two sizes;
   - cancellation within a time bound;
   - the time and memory budgets.
8. Tests:
   - every built-in plug-in passes the same validation suite that external
     authors run;
   - golden-image regression tests for the converted styles, proving
     conversion didn't change their output.

### Acceptance criteria

- Fresh install: all fourteen styles are present and look identical (pixel
  for pixel, for a fixed seed) to the pre-conversion output.
- Dropping a valid external plug-in into any plug-in location (D9) and
  pressing
  Rescan makes it appear in the gallery. After the user approves it once, it
  works with sliders, preview, export, progress and cancel, with no restart.
- The example Rust and Python plug-ins pass the validator in CI on Linux,
  macOS and Windows.
- An invalid, crashing or looping plug-in produces a clear error and the app
  keeps running.
- The Art Director can use external plug-ins in its recipes, and unknown style
  ids in saved or AI-provided recipes are handled gracefully (a message, not a
  crash).
- `docs/plugins.md` lets a new author build, validate and install the example
  plug-in following only the document.
- CI runs the plug-in validation suite on all built-ins.

### Open questions

None. All design questions are resolved (D1–D10), so the item is **Ready**.
Details such as exact JSON field names, error codes and UI wording are
settled during implementation and documented in `docs/plugins.md`.

### Dependencies and risks

- Touches the central style registry, so it should land as a sequence of small
  pull requests:
  1. the registry and trait, with the built-ins converted;
  2. the plug-ins panel and folder discovery;
  3. the external-process runtime and trust prompt;
  4. the validator and docs.
- Converting built-ins must be provably behaviour-preserving (golden-image
  tests first).
- Plug-ins run arbitrary code with the user's permissions. The trust prompt,
  its wording and the checksum pinning are the security boundary, and need
  careful review.
- The portable `plugins` folder next to the executable may not be writable on
  system-wide installs. The user folder (D9) covers this case, and the UI
  should point users to it.
- Several plug-in locations make "which plug-in am I running?" less obvious,
  so the Plug-ins panel must always show each plug-in's source location and
  any shadowed copies.
- Supporting every contract version forever (D10) means each version must be
  kept small and deliberate. Extra versions increase the testing surface.
- Cross-platform process handling (pipes, killing process trees, Windows
  quirks) needs CI coverage on all three operating systems.

---

## BL-002 · Bridge plug-ins for G'MIC, GEGL, ImageMagick and OpenCV

**Status:** Proposed · **Depends on:** BL-001 · **Priority:** optional

### Summary

Ship example "bridge" plug-ins, built on the BL-001 contract, that expose
filters from widely used tools. That would make hundreds of existing effects
available as Photo·AIrt styles without reimplementing them.

### Ideas

- **G'MIC bridge:** expose a curated set of G'MIC artistic filters
  (painting, sketch, cartoon and similar), each with its settings mapped to
  our schema, by calling the `gmic` CLI.
- **GEGL bridge:** expose GIMP's GEGL operations (`gegl:cartoon`,
  `gegl:oilify`, `gegl:photocopy` and others) through the `gegl` CLI.
- **ImageMagick bridge:** artistic operators (`-paint`, `-sketch`,
  `-charcoal`, `-posterize` and combinations) through `magick`.
- **OpenCV bridge:** Python plug-ins using `cv2.stylization`,
  `cv2.pencilSketch` and similar non-photorealistic filters.

### Constraints

- Bridges are optional external plug-ins. They require the user to have the
  tool installed and never ship inside the single executable (D3).
- Each bridge reports clearly in `describe` or `error` when its tool is
  missing.
- Respect each tool's licence. Calling them as separate programs keeps
  Photo·AIrt's MIT licence unaffected, but it should still be documented.
  GEGL is LGPL; G'MIC is CeCILL (GPL-compatible); ImageMagick uses its own
  Apache-style licence; OpenCV is Apache-2.0.
- Filters must still meet the BL-001 requirements: same output size,
  deterministic for a seed, and scaled settings.

### Open questions

- Which filters to expose first, and how to map each tool's parameters to
  `number`, `toggle` and `choice` settings.
- Should a bridge expose one plug-in per filter, or one plug-in with many
  "styles"? This may need BL-001's `describe` to allow several styles per
  executable.

---

_Add new items below this line._
