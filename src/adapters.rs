//! Composition root for trusted host adapters selected at build time.
pub struct Adapter {
    pub id: &'static str,
    pub inputs: &'static [&'static str],
    pub snapshot: &'static str,
    pub output: &'static str,
    pub verify: fn(&[u8]) -> anyhow::Result<()>,
}

#[cfg(feature = "krita-experiment")]
#[path = "../examples/krita-format/validator.rs"]
mod krita_format;

pub static INSTALLED: &[&Adapter] = &[
    #[cfg(feature = "krita-experiment")]
    &krita_format::ADAPTER,
];

pub fn named(name: Option<&str>) -> Option<&'static Adapter> {
    INSTALLED.iter().copied().find(|a| Some(a.id) == name)
}
