//! The fourteen styles compiled into the executable (decision D3), exposed
//! through the same [`StylePlugin`] contract as external plug-ins.
//!
//! Each lives in its own folder under `plugins/`, laid out like an external
//! plug-in: a `plugin.toml` that describes it, plus a crate built on
//! `photo-airt-sdk`. The crate embeds that `plugin.toml` as its description,
//! and the app links the crate in. The same crate also builds a standalone
//! executable, so a style removed from [`STYLES`] and moved to a plug-ins
//! location loads as an external plug-in instead.

use std::sync::Arc;

use photo_airt_sdk::{Ctx, Description, Params, Style};

use super::{RenderError, StylePlugin};
use crate::imaging::Img;

/// The built-in styles, in gallery order.
const STYLES: [Style; 14] = [
    photo_airt_oil::STYLE,
    photo_airt_brush::STYLE,
    photo_airt_watercolor::STYLE,
    photo_airt_flow::STYLE,
    photo_airt_pointillism::STYLE,
    photo_airt_pencil::STYLE,
    photo_airt_ink::STYLE,
    photo_airt_toon::STYLE,
    photo_airt_stained_glass::STYLE,
    photo_airt_lowpoly::STYLE,
    photo_airt_popart::STYLE,
    photo_airt_halftone::STYLE,
    photo_airt_riso::STYLE,
    photo_airt_pixel::STYLE,
];

struct Builtin {
    desc: Description,
    render: fn(&Img, &Params, &Ctx) -> Option<Img>,
}

impl StylePlugin for Builtin {
    fn description(&self) -> &Description {
        &self.desc
    }

    fn render(&self, img: &Img, params: &Params, ctx: &Ctx) -> Result<Img, RenderError> {
        (self.render)(img, params, ctx).ok_or(RenderError::Cancelled)
    }
}

pub(super) fn all() -> Vec<Arc<dyn StylePlugin>> {
    STYLES.iter().map(|s| Arc::new(Builtin { desc: (s.description)(), render: s.render }) as Arc<dyn StylePlugin>).collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::plugins::external::Manifest;

    /// Every built-in folder is a complete external plug-in: its manifest is
    /// valid and runs the executable its crate builds.
    #[test]
    fn builtin_folders_are_shaped_like_external_plugins() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins");
        let mut folders: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into())
            .collect();
        folders.sort();
        let mut ids: Vec<String> = STYLES.iter().map(|s| (s.description)().id).collect();
        ids.sort();
        assert_eq!(folders, ids, "one folder per built-in style, named by id");
        for style in &STYLES {
            let desc = (style.description)();
            let dir = root.join(&desc.id);
            let manifest = Manifest::load(&dir).unwrap_or_else(|e| panic!("{}: {e}", desc.id));
            assert_eq!(manifest.description(), desc, "{}: the crate embeds its own plugin.toml", desc.id);
            assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"), "{}: built-in styles share the app's version", desc.id);
            let exe = if cfg!(windows) { format!("bin/{}.exe", desc.id) } else { format!("bin/{}", desc.id) };
            assert_eq!(manifest.command(), [exe], "{}: plugin.toml run", desc.id);
            let cargo = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
            assert!(cargo.contains(&format!("[[bin]]\nname = \"{}\"", desc.id)), "{}: Cargo.toml must build bin/{0}", desc.id);
        }
    }
}
