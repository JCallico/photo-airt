# Releases

The [Release workflow](../.github/workflows/release.yml) builds portable
executable archives when a version tag is pushed. It waits for every build
and test to pass, then creates a **draft** GitHub release with generated
release notes and a `SHA256SUMS.txt` file. Publication is a separate manual
step in GitHub, so you can review the downloads and notes first.

| Archive suffix | Platform | Contents |
|---|---|---|
| `linux-x86_64.tar.gz` | Linux x64, glibc 2.35 or newer (Ubuntu 22.04 baseline) | `photo-airt`, plus `share/` with the desktop entry and icon theme |
| `macos-aarch64.tar.gz` | macOS, Apple Silicon | `Photo-AIrt.app` |
| `macos-x86_64.tar.gz` | macOS, Intel | `Photo-AIrt.app` |
| `windows-x86_64.zip` | Windows x64 | `photo-airt.exe`, with its icon embedded |

Each archive includes the application and font licenses. Fonts and the icon
are embedded in the executable; no Rust toolchain or files beside the
executable are needed to run it. The Linux `share/` folder and the macOS app
bundle only let the desktop show the app with its name and icon. Linux still
needs its desktop graphics libraries and XDG desktop portal. The README's
[Install](../README.md#install) section has the per-OS install steps.

There is no installer, code signing with a developer identity or
notarization. The macOS bundle is signed ad hoc by
[`packaging/macos/bundle.sh`](../packaging/macos/bundle.sh), so Gatekeeper
blocks a downloaded copy until its quarantine attribute is cleared, and
Windows SmartScreen may warn about the unsigned executable. The macOS builds
currently use macOS 15 runners (Apple Silicon and Intel) and target macOS 11
and later (the Rust default and the bundle's `LSMinimumSystemVersion`); test
older systems before claiming compatibility. AI features continue to use
separately installed, logged-in Claude Code or Codex CLIs.

## The app icon

The icon's design lives in [`assets/icon/build.py`](../assets/icon/build.py),
which writes the SVG sources in `assets/icon/` and every packaged raster: the
Windows `photo-airt.ico` (embedded by `build.rs`), the macOS
`photo-airt.icns` and the Linux icon theme under `packaging/linux/share/`. The
app renders the in-app logo and window icon from the same SVGs at run time.
After changing the design, run it (it needs Python 3 and `rsvg-convert`) and
commit the regenerated files:

```bash
python3 assets/icon/build.py
```

## Preparing a release

1. Update the version everywhere it appears, then regenerate `Cargo.lock`:
   the app's `Cargo.toml` (its own version and the `version` of each path
   dependency), `sdk/Cargo.toml`, and each built-in style's `Cargo.toml` and
   `plugin.toml` under `plugins/`. A unit test fails if a built-in style's
   `plugin.toml` version differs from the app's. Merge the change through a
   pull request after the required CI checks pass.
2. Fetch `main` and tag the intended commit on `origin/main`. The tag must
   exactly match the package version, prefixed with `v` (for example,
   `v0.1.0`). For a prerelease, use a package version such as `0.2.0-beta.1`
   and a matching `v0.2.0-beta.1` tag.

   ```bash
   git fetch origin main
   git tag -a v0.1.0 origin/main -m "Photo·AIrt v0.1.0"
   git push origin v0.1.0
   ```

3. Watch **Actions → Release**. The workflow rejects branch runs, version
   mismatches and commits outside `main`'s history. It builds and tests with
   `Cargo.lock` enforced on all four native runners.
4. Open the draft under **Releases**, review the notes, download and try each
   platform's executable, and verify the checksums. Check that the icon shows
   in the window, taskbar or Dock, the app launcher and the file manager. On
   Linux:

   ```bash
   sha256sum --check --ignore-missing SHA256SUMS.txt
   tar -xzf photo-airt-v0.1.0-linux-x86_64.tar.gz
   ./photo-airt-v0.1.0-linux-x86_64/photo-airt
   ```

5. Publish the draft in GitHub. Tags containing a hyphen automatically mark
   the draft as a prerelease.

Failed runs can be rerun from Actions. To rebuild an existing tag manually,
use the GitHub CLI:

```bash
gh workflow run release.yml --ref v0.1.0
```

A successful rerun replaces assets only while the release is still a draft.
Published releases are never overwritten; prepare a new version instead.
Only the final release job receives `contents: write`; the other jobs use
read-only tokens, and all actions are GitHub-owned and pinned by commit SHA.
