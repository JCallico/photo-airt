//! The style contract shared by the host and every plug-in: the
//! self-description (D4), setting values, the render context and the wire
//! types of the external protocol (D5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::imaging::Img;

/// The contract version spoken here. Published versions stay supported
/// forever (D10); this is the only one so far.
pub const PROTOCOL: u32 = 1;

/// Size-like parameters are authored for a photo whose long side is this many
/// pixels; renders at other resolutions scale them so thumbnails, previews and
/// full-resolution exports all look alike.
pub const REFERENCE_LONG_SIDE: f32 = 1600.0;

// ------------------------------------------------------------------ settings schema

/// A setting value. Numbers drive sliders, booleans toggles, strings choices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Number(f32),
    Text(String),
}

/// One option of a `choice` setting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
}

/// The kind of control a setting needs (decision D6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    /// A slider. A step of 1 or more makes it an integer slider.
    Number {
        min: f32,
        max: f32,
        #[serde(default)]
        step: f32,
    },
    Toggle,
    Choice {
        options: Vec<ChoiceOption>,
    },
}

/// One control in the Atelier panel. `key` is part of the public contract:
/// saved recipes and the Art Director refer to settings by key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(flatten)]
    pub kind: SettingKind,
    pub default: Value,
}

impl Setting {
    /// A slider from `min` to `max`; a `step` of 1 or more makes it an integer slider.
    pub fn number(key: &str, label: &str, min: f32, max: f32, default: f32, step: f32) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            help: None,
            kind: SettingKind::Number { min, max, step },
            default: Value::Number(default),
        }
    }

    /// Coerce an arbitrary (for example AI-provided) value into this
    /// setting's domain, or `None` if it cannot be interpreted.
    pub fn coerce(&self, v: &Value) -> Option<Value> {
        match (&self.kind, v) {
            (SettingKind::Number { min, max, step }, Value::Number(n)) if n.is_finite() => {
                let mut n = n.clamp(*min, *max);
                if *step >= 1.0 {
                    n = n.round();
                }
                Some(Value::Number(n))
            }
            (SettingKind::Toggle, Value::Bool(b)) => Some(Value::Bool(*b)),
            (SettingKind::Toggle, Value::Number(n)) => Some(Value::Bool(*n != 0.0)),
            (SettingKind::Choice { options }, Value::Text(t)) => options.iter().any(|o| &o.value == t).then(|| Value::Text(t.clone())),
            _ => None,
        }
    }
}

/// What a plug-in tells the app about itself (decision D4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Description {
    /// Stable, unique id. Never changes between versions.
    pub id: String,
    pub name: String,
    /// Group in the style gallery.
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub blurb: String,
    /// Shown under "How it works".
    #[serde(default)]
    pub technique: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub licence: String,
    #[serde(default)]
    pub settings: Vec<Setting>,
}

impl Description {
    /// Check the description against the plug-in contract.
    pub fn validate(&self) -> Result<(), String> {
        let id_ok = !self.id.is_empty() && self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !id_ok {
            return Err(format!("invalid id {:?}: use lowercase letters, digits, '-' and '_'", self.id));
        }
        if self.name.trim().is_empty() {
            return Err(format!("{}: missing name", self.id));
        }
        let mut keys = std::collections::HashSet::new();
        for s in &self.settings {
            if s.key.is_empty() || !keys.insert(s.key.as_str()) {
                return Err(format!("{}: empty or duplicate setting key {:?}", self.id, s.key));
            }
            let ok = match (&s.kind, &s.default) {
                (SettingKind::Number { min, max, step }, Value::Number(d)) => min <= max && *step >= 0.0 && (min..=max).contains(&d),
                (SettingKind::Toggle, Value::Bool(_)) => true,
                (SettingKind::Choice { options }, Value::Text(d)) => {
                    let mut seen = std::collections::HashSet::new();
                    !options.is_empty() && options.iter().all(|o| seen.insert(o.value.as_str())) && options.iter().any(|o| &o.value == d)
                }
                _ => false,
            };
            if !ok {
                return Err(format!("{}: setting {:?} has an invalid range, options or default", self.id, s.key));
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ render inputs

/// Setting values for one render, keyed by setting key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Params(pub BTreeMap<String, Value>);

impl Params {
    /// Every setting at its default value.
    pub fn defaults(desc: &Description) -> Self {
        Params(desc.settings.iter().map(|s| (s.key.clone(), s.default.clone())).collect())
    }

    /// Merge externally provided values (saved recipes, the Art Director, a
    /// render request) over the defaults, coercing each into its setting's
    /// domain and ignoring unknown keys.
    pub fn sanitized(desc: &Description, raw: &BTreeMap<String, Value>) -> Self {
        let mut out = Self::defaults(desc);
        for s in &desc.settings {
            if let Some(v) = raw.get(&s.key).and_then(|v| s.coerce(v)) {
                out.0.insert(s.key.clone(), v);
            }
        }
        out
    }

    /// A numeric setting (toggles read as 0 or 1).
    pub fn get(&self, key: &str) -> f32 {
        match self.0.get(key) {
            Some(Value::Number(v)) => *v,
            Some(Value::Bool(b)) => f32::from(u8::from(*b)),
            _ => panic!("unknown numeric param {key}"),
        }
    }

    /// A toggle setting.
    pub fn flag(&self, key: &str) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(v)) => *v != 0.0,
            _ => panic!("unknown toggle param {key}"),
        }
    }

    /// A choice setting.
    pub fn choice(&self, key: &str) -> &str {
        match self.0.get(key) {
            Some(Value::Text(t)) => t,
            _ => panic!("unknown choice param {key}"),
        }
    }
}

/// Render context: resolution scaling, seed, progress and cancellation.
#[derive(Clone)]
pub struct Ctx {
    pub scale: f32,
    pub seed: u64,
    pub progress: Arc<AtomicU32>,
    pub cancel: Arc<AtomicBool>,
}

impl Ctx {
    pub fn for_image(img: &Img, seed: u64) -> Self {
        Self {
            scale: img.long_side() as f32 / REFERENCE_LONG_SIDE,
            seed,
            progress: Arc::new(AtomicU32::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    /// Scale a reference-resolution pixel size to this render.
    pub fn px(&self, v: f32) -> f32 {
        (v * self.scale).max(0.5)
    }
    pub fn progress(&self, f: f32) {
        self.progress.store((f.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }
    /// Progress reported so far, from 0 to 1.
    pub fn progress_value(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn seed32(&self) -> u32 {
        (self.seed ^ (self.seed >> 32)) as u32
    }
}

/// A style compiled from Rust: its description (read from its own
/// `plugin.toml`, see [`Manifest::embedded`]) and its pure render function.
/// Must be deterministic for a given seed, return an image the size of its
/// input, scale size-like settings with [`Ctx::px`], report progress and
/// return `None` once [`Ctx::cancelled`].
#[derive(Clone, Copy)]
pub struct Style {
    pub description: fn() -> Description,
    pub render: fn(&Img, &Params, &Ctx) -> Option<Img>,
}

// ------------------------------------------------------------------ manifest

/// `plugin.toml`: the whole description of a plug-in (D4). The app reads it
/// without running anything, so users see what they are approving.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Contract versions the plug-in speaks (a number or a list).
    #[serde(deserialize_with = "one_or_many")]
    pub protocol: Vec<u32>,
    /// Stable, unique id. Never changes between versions.
    pub id: String,
    pub name: String,
    /// Group in the style gallery.
    #[serde(default = "default_family")]
    pub family: String,
    #[serde(default)]
    pub blurb: String,
    /// Shown under "How it works".
    #[serde(default)]
    pub technique: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub licence: String,
    /// Longest a render may take; the host applies its default and maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Command and arguments. The first element is a program on `PATH` or a
    /// file inside the plug-in folder.
    pub run: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_windows: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_macos: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_linux: Option<Vec<String>>,
    #[serde(default)]
    pub settings: Vec<Setting>,
}

fn default_family() -> String {
    "Plug-ins".into()
}

fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(u32),
        Many(Vec<u32>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(v) => vec![v],
        OneOrMany::Many(v) => v,
    })
}

impl Manifest {
    /// The manifest's file name inside a plug-in folder.
    pub const FILE: &str = "plugin.toml";

    /// Parse and check a manifest.
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| format!("invalid {}: {e}", Self::FILE))?;
        m.check()?;
        Ok(m)
    }

    /// Read, parse and check `<dir>/plugin.toml`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(dir.join(Self::FILE)).map_err(|e| format!("cannot read {}: {e}", Self::FILE))?;
        Self::parse(&text)
    }

    /// The description of a style compiled from Rust, from its own
    /// `plugin.toml` embedded with `include_str!`. Panics if it is invalid,
    /// which the style's tests catch.
    pub fn embedded(text: &str) -> Description {
        Self::parse(text).unwrap_or_else(|e| panic!("{e}")).description()
    }

    /// Check the manifest against the contract.
    pub fn check(&self) -> Result<(), String> {
        if !self.protocol.contains(&PROTOCOL) {
            return Err(format!("speaks contract versions {:?}, this app speaks {PROTOCOL}", self.protocol));
        }
        if self.command().is_empty() || self.command()[0].trim().is_empty() {
            return Err(format!("{} needs a non-empty run command", Self::FILE));
        }
        self.description().validate()
    }

    /// What the gallery and the Atelier panel show.
    pub fn description(&self) -> Description {
        Description {
            id: self.id.clone(),
            name: self.name.clone(),
            family: self.family.clone(),
            blurb: self.blurb.clone(),
            technique: self.technique.clone(),
            version: self.version.clone(),
            author: self.author.clone(),
            licence: self.licence.clone(),
            settings: self.settings.clone(),
        }
    }

    /// The command for this operating system.
    pub fn command(&self) -> &[String] {
        let os = if cfg!(windows) {
            &self.run_windows
        } else if cfg!(target_os = "macos") {
            &self.run_macos
        } else {
            &self.run_linux
        };
        os.as_deref().unwrap_or(&self.run)
    }
}

// ------------------------------------------------------------------ wire types

/// `request.json` for one render (D5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderRequest {
    pub protocol: u32,
    pub plugin_id: String,
    /// Absolute path of the input PNG (8-bit RGB).
    pub input: PathBuf,
    /// Absolute path where the plug-in must write its PNG result.
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Current value of every declared setting, by key.
    pub params: BTreeMap<String, Value>,
    /// Drives any randomness: same seed, same picture. Below 2^53.
    pub seed: u64,
    /// This image's long side relative to the reference size (1600 px).
    pub scale: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_params_coerce_each_kind() {
        let desc: Description = serde_json::from_str(
            r#"{
                "id": "demo", "name": "Demo",
                "settings": [
                    {"key": "size", "label": "Size", "type": "number", "min": 2, "max": 24, "step": 1, "default": 7},
                    {"key": "grain", "label": "Grain", "type": "toggle", "default": false},
                    {"key": "inks", "label": "Inks", "type": "choice", "default": "pink-blue",
                     "options": [{"value": "pink-blue", "label": "Pink + blue"}, {"value": "teal-orange", "label": "Teal + orange"}]}
                ]
            }"#,
        )
        .unwrap();
        desc.validate().unwrap();
        let raw: BTreeMap<String, Value> =
            serde_json::from_str(r#"{"size": 99.4, "grain": true, "inks": "teal-orange", "bogus": 1}"#).unwrap();
        let p = Params::sanitized(&desc, &raw);
        assert_eq!(p.get("size"), 24.0);
        assert!(p.flag("grain"));
        assert_eq!(p.choice("inks"), "teal-orange");
        assert!(!p.0.contains_key("bogus"));
        // Unknown choice values and wrong types fall back to defaults.
        let raw: BTreeMap<String, Value> = serde_json::from_str(r#"{"size": "big", "inks": "magenta"}"#).unwrap();
        let p = Params::sanitized(&desc, &raw);
        assert_eq!(p.get("size"), 7.0);
        assert_eq!(p.choice("inks"), "pink-blue");
        // Descriptions round-trip through JSON.
        let json = serde_json::to_string(&desc).unwrap();
        assert_eq!(serde_json::from_str::<Description>(&json).unwrap(), desc);
    }

    #[test]
    fn invalid_descriptions_are_rejected() {
        let bad = |json: &str| serde_json::from_str::<Description>(json).unwrap().validate().is_err();
        assert!(bad(r#"{"id": "Bad Id", "name": "x"}"#));
        assert!(bad(
            r#"{"id": "x", "name": "x", "settings": [{"key": "a", "label": "A", "type": "number", "min": 0, "max": 1, "default": 5}]}"#
        ));
        assert!(bad(r#"{"id": "x", "name": "x", "settings": [{"key": "a", "label": "A", "type": "toggle", "default": 1}]}"#));
        assert!(bad(
            r#"{"id": "x", "name": "x", "settings": [{"key": "a", "label": "A", "type": "choice", "options": [], "default": "z"}]}"#
        ));
        assert!(bad(
            r#"{"id": "x", "name": "x", "settings": [{"key": "a", "label": "A", "type": "toggle", "default": true}, {"key": "a", "label": "B", "type": "toggle", "default": true}]}"#
        ));
    }

    #[test]
    fn manifests_describe_the_plugin() {
        let m = Manifest::parse(
            r#"
protocol = 1
id = "demo"
name = "Demo"
run = ["python3", "demo.py"]
run_windows = ["python", "demo.py"]

[[settings]]
key = "size"
label = "Size"
type = "number"
min = 2
max = 24
step = 1
default = 7

[[settings]]
key = "inks"
label = "Inks"
type = "choice"
default = "a"
options = [{ value = "a", label = "A" }, { value = "b", label = "B" }]
"#,
        )
        .unwrap();
        assert_eq!(m.command()[0], if cfg!(windows) { "python" } else { "python3" });
        let d = m.description();
        assert_eq!(d.family, "Plug-ins", "family defaults to a generic group");
        assert_eq!(Params::defaults(&d).get("size"), 7.0);
        assert_eq!(Params::defaults(&d).choice("inks"), "a");

        let base = "id = \"x\"\nname = \"X\"\nrun = [\"x\"]\n";
        assert!(Manifest::parse(&format!("protocol = [1, 2]\n{base}")).is_ok());
        assert!(Manifest::parse(&format!("protocol = [2, 3]\n{base}")).is_err(), "unsupported contract");
        assert!(Manifest::parse(base).is_err(), "protocol is required");
        assert!(Manifest::parse(&format!("protocol = 1\n{base}surprise = 1\n")).is_err(), "unknown keys are rejected");
        assert!(Manifest::parse("protocol = 1\nid = \"x\"\nname = \"X\"\nrun = []\n").is_err(), "run is required");
        assert!(Manifest::parse(&format!("protocol = 1\n{base}").replace("\"x\"\nname", "\"Bad Id\"\nname")).is_err());
    }
}
