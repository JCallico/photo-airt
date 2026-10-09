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

**Status:** Design · **Area:** `src/styles/`, `src/app.rs`, UI, docs

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
4. **Safe by default.** A plug-in cannot read files, use the network or run
   programs, and a crashing or runaway plug-in cannot take the app down.
5. **Documented.** An authoring guide with a working example, the full
   requirements list and a validation tool.

### Requirements for the plug-in contract

These are the requirements every plug-in must meet to work with the
application. They are derived from how the built-in styles behave today and
become the documented contract.

**Identity and metadata (manifest)**

- A stable, unique `id`. It is referenced by saved recipes, `recents`, AI
  output and the command line, so it must never change between versions.
- Display `name`, `family` (group in the gallery), a one-line `blurb`, a
  `technique` description (shown under "How it works"), `version`, author and
  licence, and the plug-in API version it targets.
- A parameter schema: for each parameter a `key`, label, minimum, maximum,
  default and step. A step of 1 or more means an integer slider. Defaults must
  lie within range. Parameters are plain numbers so they can be sliders and be
  set by the Art Director.

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

- No filesystem, network, process or environment access.
- A trap, panic or timeout in a plug-in is reported as a toast and a failed
  render. It must never crash the app or freeze the UI.
- Input photos are untrusted content. Plug-ins must not assume a size or a
  value range beyond the documented ones.

**Distribution**

- A plug-in is a self-contained package, a folder or single file with its
  manifest, in a per-user plug-ins folder.
- Loading is automatic at startup and via a "Rescan" action, and invalid
  plug-ins are reported with a clear reason instead of being skipped silently.
- Built-in plug-ins are embedded in the binary and can't be removed, but can
  be shadowed or disabled.

### Design approach to decide

The central decision is **how external plug-ins execute**. Candidates, with
the trade-offs that matter for this project:

| Approach | Safety | Portability | Authoring experience | Performance | Notes |
|---|---|---|---|---|---|
| **WebAssembly** (for example `wasmtime`, or `extism`, or `wasmi`) | Strong sandbox by construction (no ambient access, memory and fuel limits) | One `.wasm` file runs on every OS and CPU | Any language that compiles to Wasm; Rust is first-class | Near-native; SIMD and threads need care | Best fit for "safe and zero setup". Needs a stable host ABI for passing pixel buffers. Adds a sizeable dependency. |
| **Native dynamic libraries** (`libloading` plus a stable ABI crate) | None: a plug-in is arbitrary native code | Per-OS and per-CPU binaries; Rust ABI instability | Rust (or C) | Native | Fastest, but the weakest on safety and distribution. Not recommended for untrusted plug-ins. |
| **External process** (a plug-in is an executable speaking a small protocol over stdin/stdout or shared memory, like our CLI integrations) | OS-level sandboxing possible, but not by default | Per-OS binaries | Any language | Good; adds copying overhead | Familiar pattern in this codebase. Weaker isolation unless sandboxed. |
| **Embedded scripting** (for example `rhai`, Lua) | Good (no I/O unless exposed) | Fully portable text files | Easy to write, easy to share | Slow for per-pixel work unless the host exposes fast primitives | Best for compositions of existing primitives. Poor for new low-level algorithms. |
| **Declarative recipes** (parameterised pipelines over built-in primitives, in data files) | Excellent | Fully portable | Easiest; no code | Native (the host does the work) | Limited to what the primitives can express. A good *additional* tier. |

**Provisional recommendation (to be confirmed):** a layered design.

1. A Rust **trait/ABI-neutral host interface** (`StylePlugin`) that all
   built-ins implement. This is the contract in code.
2. **WebAssembly** as the execution model for external plug-ins, behind the
   same interface, using one of the maintained runtimes.
3. Optionally, **declarative recipe plug-ins** later, composing the host's
   primitives (flow fields, blurs, XDoG, Voronoi and so on), which gives
   non-programmers a path in.

The built-ins stay native Rust for speed, but are registered through the same
registry, manifest and parameter schema as external plug-ins.

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
5. The execution sandbox for external plug-ins (once the approach is chosen):
   resource limits, timeouts and error isolation.
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
- Dropping a valid external plug-in into the plug-ins folder and pressing
  Rescan makes it appear in the gallery with sliders, preview and export, with
  no restart.
- An invalid, crashing or looping plug-in produces a clear error and the app
  keeps running.
- The Art Director can use external plug-ins in its recipes, and unknown style
  ids in saved or AI-provided recipes are handled gracefully (a message, not a
  crash).
- `docs/plugins.md` lets a new author build, validate and install the example
  plug-in following only the document.
- CI runs the plug-in validation suite on all built-ins.

### Open questions (need a decision before building)

1. **Execution model for external plug-ins:** WebAssembly, an external
   process, scripting, or declarative only? (See the table. The provisional
   recommendation is WebAssembly plus built-ins as native Rust.)
2. **Which runtime crate**, if WebAssembly: `wasmtime` (fastest, largest),
   `wasmi` (small, interpreter, slower), or `extism` (a higher-level plug-in
   framework)? This needs a maintenance, binary-size and licence review under
   our `cargo-deny` policy.
3. **Pixel interface:** raw RGB float buffers across the boundary, or tiles
   and rows? Does the host expose primitive operations (blur, flow fields,
   noise) to plug-ins so they don't each reimplement them?
4. **Trust model:** is a plug-in always sandboxed, or can a user opt in to
   trusted native plug-ins? Should plug-ins be signed or checksummed?
5. **Name and location of the plug-ins folder** on each OS, and whether the
   project hosts a curated plug-in index.
6. **Compatibility policy:** how long is a plug-in API version supported, and
   how are breaking changes announced?

### Dependencies and risks

- Touches the central style registry, so it should land as a sequence of small
  pull requests:
  1. the registry and trait, with the built-ins converted;
  2. the plug-ins panel and folder discovery;
  3. the sandboxed external runtime;
  4. the validator and docs.
- Converting built-ins must be provably behaviour-preserving (golden-image
  tests first).
- A Wasm runtime adds binary size and build time. Measure before committing.

---

_Add new items below this line._
