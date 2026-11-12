use crate::backend::{self, BackendConfig, BackendError, BackendKind};
use crate::identify::{self, Selection};
use crate::plan::AskReason;
use crate::render::{self, LayerPng};
use crate::strokes::{self, Component};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ready {
        attempt: String,
        revision: u64,
        recognized: Option<String>,
        #[serde(skip_serializing)]
        layers: Vec<LayerPng>,
    },
    Ask {
        reason: AskReason,
        detail: String,
    },
    Rejected {
        reason: RejectReason,
    },
    Failed {
        reason: String,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Stale,
    Locked,
}

#[derive(Debug)]
pub enum CommitError {
    NotLocked,
    AttemptMismatch,
    Io(String),
}

pub struct RenderRequest {
    pub revision: u64,
    pub backend: BackendKind,
    pub description: String,
    pub strokes_png: Vec<u8>,
    pub selection: Option<Selection>,
}

pub struct Document {
    root: PathBuf,
    revision: u64,
    lock: Option<String>,
    strokes: Option<Vec<u8>>,
    layers: Vec<LayerPng>,
    backends: BackendConfig,
}

impl Document {
    pub fn open(root: &Path) -> Result<Self, String> {
        let root = root.to_path_buf();
        let state = root.join("state/default");
        fs::create_dir_all(state.join("layers")).map_err(|e| e.to_string())?;
        let revision = read_text(&state.join("revision"))
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0);
        let lock = fs::read_to_string(state.join("lock.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| {
                value
                    .get("attempt")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        let strokes = read_bytes(&state.join("strokes.png"));
        let layers = load_layers(&live_or_backup(&state.join("layers")))?;
        let backends = backend::load_config(&root);
        Ok(Self {
            root,
            revision,
            lock,
            strokes,
            layers,
            backends,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn locked(&self) -> bool {
        self.lock.is_some()
    }

    pub fn render(&mut self, request: &RenderRequest) -> Outcome {
        if self.lock.is_some() {
            return Outcome::Rejected {
                reason: RejectReason::Locked,
            };
        }
        if request.revision != self.revision {
            return Outcome::Rejected {
                reason: RejectReason::Stale,
            };
        }
        if request.description.trim().is_empty() {
            return Outcome::Ask {
                reason: AskReason::EmptyDescription,
                detail: "要渲染成什么样子".into(),
            };
        }
        let image = match strokes::decode_png(&request.strokes_png) {
            Ok(image) => image,
            Err(error) => return failed(error),
        };
        let previous = match &self.strokes {
            Some(bytes) => match strokes::decode_png(bytes) {
                Ok(image) => Some(image),
                Err(error) => return failed(error),
            },
            None => None,
        };
        let attempt = new_attempt();
        let task = backend::task_dir(&self.root, &attempt);
        if let Err(error) = fs::create_dir_all(&task) {
            return failed(error.to_string());
        }
        let prepared = if let Some(selection) = request.selection {
            self.prepare_edit(&image, previous.as_ref(), selection, request, &task)
        } else {
            self.prepare_full(&image, previous.as_ref(), request, &task)
        };
        let (plan, components, replace_at, recognized) = match prepared {
            Ok(value) => value,
            Err(outcome) => {
                let _ = fs::remove_dir_all(&task);
                return outcome;
            }
        };
        let layers = match render::paint(
            image.width(),
            image.height(),
            &plan,
            &components,
            &self.layers,
            replace_at,
        ) {
            Ok(layers) => layers,
            Err(error) => {
                let _ = fs::remove_dir_all(&task);
                return failed(format!("{error:?}"));
            }
        };
        if let Err(error) = store_attempt(&task, &request.strokes_png, &layers) {
            let _ = fs::remove_dir_all(&task);
            return failed(error);
        }
        if let Err(error) = fs::write(
            self.root.join("state/default/lock.json"),
            format!("{{\"attempt\":\"{attempt}\"}}"),
        ) {
            let _ = fs::remove_dir_all(&task);
            return failed(error.to_string());
        }
        self.lock = Some(attempt.clone());
        Outcome::Ready {
            attempt,
            revision: self.revision,
            recognized,
            layers,
        }
    }

    pub fn commit(&mut self, attempt: &str) -> Result<u64, CommitError> {
        match &self.lock {
            Some(current) if current == attempt => {}
            Some(_) => return Err(CommitError::AttemptMismatch),
            None => return Err(CommitError::NotLocked),
        }
        let task = backend::task_dir(&self.root, attempt);
        let strokes =
            fs::read(task.join("strokes.png")).map_err(|e| CommitError::Io(e.to_string()))?;
        let layers = load_layers(&task.join("layers")).map_err(CommitError::Io)?;
        let next = self.revision + 1;
        publish(&self.root.join("state/default"), &strokes, &layers, next)
            .map_err(CommitError::Io)?;
        self.revision = next;
        self.strokes = Some(strokes);
        self.layers = layers;
        match fs::remove_file(self.root.join("state/default/lock.json")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(CommitError::Io(error.to_string())),
        }
        let _ = fs::remove_dir_all(&task);
        self.lock = None;
        Ok(self.revision)
    }

    pub fn abort(&mut self, _attempt: &str) -> Result<(), CommitError> {
        if let Some(attempt) = self.lock.take() {
            let _ = fs::remove_file(self.root.join("state/default/lock.json"));
            let _ = fs::remove_dir_all(backend::task_dir(&self.root, &attempt));
        }
        Ok(())
    }

    fn prepare_full(
        &self,
        image: &image::RgbaImage,
        previous: Option<&image::RgbaImage>,
        request: &RenderRequest,
        task: &Path,
    ) -> Result<
        (
            crate::plan::RenderPlan,
            Vec<Component>,
            Option<usize>,
            Option<String>,
        ),
        Outcome,
    > {
        let found = strokes::components(image, previous);
        let mut calls = 0;
        match backend::run(
            request.backend,
            task,
            &self.backends,
            &request.description,
            found.len(),
            &mut calls,
        ) {
            Ok(plan) => Ok((plan, found, None, None)),
            Err(BackendError::Ask(reason)) => Err(ask(reason)),
            Err(BackendError::Failed(detail)) => Err(failed(detail)),
        }
    }

    fn prepare_edit(
        &self,
        image: &image::RgbaImage,
        previous: Option<&image::RgbaImage>,
        selection: Selection,
        request: &RenderRequest,
        task: &Path,
    ) -> Result<
        (
            crate::plan::RenderPlan,
            Vec<Component>,
            Option<usize>,
            Option<String>,
        ),
        Outcome,
    > {
        if self.layers.is_empty() {
            return Err(ask(AskReason::UnknownObject));
        }
        let masks = self
            .layers
            .iter()
            .map(|layer| {
                strokes::decode_png(&layer.png)
                    .map(|decoded| strokes::opaque_pixels(&decoded))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let index = match identify::identify(&masks, selection) {
            Ok(index) => index,
            Err(reason) => return Err(ask(reason)),
        };
        let mut pixels = masks[index].clone();
        for (x, y, pixel) in image.enumerate_pixels() {
            if pixel[3] < 16 || !contains(selection, x, y) {
                continue;
            }
            let old = previous
                .filter(|image| x < image.width() && y < image.height())
                .map(|image| image.get_pixel(x, y)[3])
                .unwrap_or(0);
            if old < 16 && !pixels.contains(&(x, y)) {
                pixels.push((x, y));
            }
        }
        let component = Component {
            index: 0,
            pixels,
            bounds: (0, 0, 0, 0),
        };
        let mut calls = 0;
        let plan = match backend::run(
            request.backend,
            task,
            &self.backends,
            &request.description,
            1,
            &mut calls,
        ) {
            Ok(plan) => plan,
            Err(BackendError::Ask(reason)) => return Err(ask(reason)),
            Err(BackendError::Failed(detail)) => return Err(failed(detail)),
        };
        let recognized = self.layers[index]
            .name
            .trim_start_matches("an/")
            .to_string();
        Ok((plan, vec![component], Some(index), Some(recognized)))
    }
}

fn ask(reason: AskReason) -> Outcome {
    let detail = match reason {
        AskReason::EmptyDescription => "要渲染成什么样子",
        AskReason::CountMismatch => "笔触块数和句子段数对不上",
        AskReason::UnknownObject => "看不出是哪一个物体",
    };
    Outcome::Ask {
        reason,
        detail: detail.into(),
    }
}

fn failed(detail: String) -> Outcome {
    Outcome::Failed {
        reason: "backend".into(),
        detail,
    }
}

fn contains(selection: Selection, x: u32, y: u32) -> bool {
    x >= selection.x
        && y >= selection.y
        && x < selection.x.saturating_add(selection.w)
        && y < selection.y.saturating_add(selection.h)
}

fn new_attempt() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

const PUBLISHED: [&str; 3] = ["strokes.png", "layers", "revision"];

fn publish(state: &Path, strokes: &[u8], layers: &[LayerPng], revision: u64) -> Result<(), String> {
    clear_suffix(state, "next");
    fs::write(sibling(&state.join("strokes.png"), "next"), strokes).map_err(|e| e.to_string())?;
    store_layers(&sibling(&state.join("layers"), "next"), layers)?;
    fs::write(
        sibling(&state.join("revision"), "next"),
        revision.to_string(),
    )
    .map_err(|e| e.to_string())?;
    if let Err(error) = park(state) {
        restore(state);
        clear_suffix(state, "next");
        return Err(error);
    }
    if let Err(error) = install(state) {
        for name in PUBLISHED {
            remove_path(&state.join(name));
        }
        restore(state);
        clear_suffix(state, "next");
        return Err(error);
    }
    clear_suffix(state, "bak");
    Ok(())
}

fn park(state: &Path) -> Result<(), String> {
    for name in PUBLISHED {
        let live = state.join(name);
        if !live.exists() {
            continue;
        }
        let backup = sibling(&live, "bak");
        remove_path(&backup);
        fs::rename(&live, &backup).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn install(state: &Path) -> Result<(), String> {
    for name in PUBLISHED {
        fs::rename(sibling(&state.join(name), "next"), state.join(name))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn restore(state: &Path) {
    for name in PUBLISHED {
        let live = state.join(name);
        let backup = sibling(&live, "bak");
        if !backup.exists() {
            continue;
        }
        remove_path(&live);
        let _ = fs::rename(&backup, &live);
    }
}

fn clear_suffix(state: &Path, suffix: &str) {
    for name in PUBLISHED {
        remove_path(&sibling(&state.join(name), suffix));
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("item");
    path.with_file_name(format!("{name}.{suffix}"))
}

fn live_or_backup(path: &Path) -> PathBuf {
    if path.join("manifest.json").exists() {
        return path.to_path_buf();
    }
    let backup = sibling(path, "bak");
    if backup.join("manifest.json").exists() {
        backup
    } else {
        path.to_path_buf()
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .or_else(|| fs::read_to_string(sibling(path, "bak")).ok())
}

fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    fs::read(path)
        .ok()
        .or_else(|| fs::read(sibling(path, "bak")).ok())
}

fn remove_path(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else if path.exists() {
        let _ = fs::remove_file(path);
    }
}

fn store_attempt(task: &Path, strokes: &[u8], layers: &[LayerPng]) -> Result<(), String> {
    write_bytes(&task.join("strokes.png"), strokes)?;
    store_layers(&task.join("layers"), layers)
}

fn store_layers(dir: &Path, layers: &[LayerPng]) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut manifest = Vec::new();
    for (index, layer) in layers.iter().enumerate() {
        let file = format!("{index}.png");
        write_bytes(&dir.join(&file), &layer.png)?;
        manifest.push(serde_json::json!({"name": layer.name, "file": file}));
    }
    write_bytes(
        &dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)
            .map_err(|e| e.to_string())?
            .as_slice(),
    )
}

fn load_layers(dir: &Path) -> Result<Vec<LayerPng>, String> {
    let manifest = dir.join("manifest.json");
    if !manifest.exists() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let mut layers = Vec::new();
    for item in value.as_array().ok_or("layer manifest is not a list")? {
        let name = item["name"].as_str().unwrap_or("").to_string();
        let file = item["file"].as_str().unwrap_or("");
        let png = fs::read(dir.join(file)).map_err(|e| e.to_string())?;
        layers.push(LayerPng { name, png });
    }
    Ok(layers)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "an-state-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn blob() -> Vec<u8> {
        let mut image = RgbaImage::new(40, 8);
        for x in 0..8 {
            image.put_pixel(x, 0, Rgba([0, 0, 0, 255]));
            image.put_pixel(x + 20, 0, Rgba([0, 0, 0, 255]));
        }
        render::encode(&image).unwrap()
    }

    fn request(revision: u64, description: &str, selection: Option<Selection>) -> RenderRequest {
        RenderRequest {
            revision,
            backend: BackendKind::Code,
            description: description.into(),
            strokes_png: blob(),
            selection,
        }
    }

    #[test]
    fn empty_sentence_does_not_lock() {
        let mut doc = Document::open(&root()).unwrap();
        let outcome = doc.render(&request(0, "  ", None));
        assert!(matches!(
            outcome,
            Outcome::Ask {
                reason: AskReason::EmptyDescription,
                ..
            }
        ));
        assert!(doc.lock.is_none());
        assert!(!doc.root.join("state/default/lock.json").exists());
    }

    #[test]
    fn render_locks_until_commit_and_rejects_a_stale_revision() {
        let path = root();
        let mut doc = Document::open(&path).unwrap();
        let ready = doc.render(&request(0, "红色轮子，黄铜握把", None));
        let Outcome::Ready {
            attempt, revision, ..
        } = ready
        else {
            panic!("ready")
        };
        assert_eq!(revision, 0);
        assert!(path.join("state/default/lock.json").exists());
        assert!(matches!(
            doc.render(&request(0, "红色轮子，黄铜握把", None)),
            Outcome::Rejected {
                reason: RejectReason::Locked
            }
        ));
        assert_eq!(doc.commit(&attempt).unwrap(), 1);
        assert!(!path.join("state/default/lock.json").exists());
        assert_eq!(
            fs::read(path.join("state/default/strokes.png")).unwrap(),
            blob()
        );
        assert!(matches!(
            doc.render(&request(0, "红色轮子，黄铜握把", None)),
            Outcome::Rejected {
                reason: RejectReason::Stale
            }
        ));
    }

    #[test]
    fn abort_and_a_wrong_attempt_keep_the_revision() {
        let path = root();
        let mut doc = Document::open(&path).unwrap();
        let Outcome::Ready { attempt, .. } = doc.render(&request(0, "红色轮子，黄铜握把", None))
        else {
            panic!("ready")
        };
        assert!(doc.commit("nope").is_err());
        assert!(doc.locked());
        doc.abort(&attempt).unwrap();
        assert_eq!(doc.revision(), 0);
        assert!(!doc.locked());
        let mut again = Document::open(&path).unwrap();
        again.render(&request(0, "红色轮子，黄铜握把", None));
        let reopened = Document::open(&path).unwrap();
        assert!(reopened.locked());
        assert!(matches!(
            Document::open(&path)
                .unwrap()
                .render(&request(0, "红色轮子，黄铜握把", None)),
            Outcome::Rejected {
                reason: RejectReason::Locked
            }
        ));
    }

    #[test]
    fn a_selection_repaints_only_the_recognized_layer() {
        let path = root();
        let mut doc = Document::open(&path).unwrap();
        let Outcome::Ready {
            attempt, layers, ..
        } = doc.render(&request(0, "红色轮子，黄铜握把", None))
        else {
            panic!("ready")
        };
        let right = layers[1].png.clone();
        doc.commit(&attempt).unwrap();
        let outcome = doc.render(&request(
            1,
            "蓝色轮子",
            Some(Selection {
                x: 0,
                y: 0,
                w: 12,
                h: 8,
            }),
        ));
        let Outcome::Ready {
            attempt,
            recognized,
            layers,
            ..
        } = outcome
        else {
            panic!("ready edit")
        };
        assert_eq!(recognized.as_deref(), Some("轮子"));
        assert_eq!(layers[1].png, right);
        assert_ne!(
            layers[0].png,
            fs::read(path.join("state/default/layers/0.png")).unwrap()
        );
        doc.commit(&attempt).unwrap();
        assert_eq!(
            fs::read(path.join("state/default/layers/1.png")).unwrap(),
            right
        );
    }

    #[test]
    fn a_blank_selection_does_not_start_an_attempt() {
        let path = root();
        let mut doc = Document::open(&path).unwrap();
        let Outcome::Ready { attempt, .. } = doc.render(&request(0, "红色轮子，黄铜握把", None))
        else {
            panic!("ready")
        };
        doc.commit(&attempt).unwrap();
        let outcome = doc.render(&request(
            1,
            "蓝色轮子",
            Some(Selection {
                x: 30,
                y: 0,
                w: 4,
                h: 2,
            }),
        ));
        assert!(matches!(
            outcome,
            Outcome::Ask {
                reason: AskReason::UnknownObject,
                ..
            }
        ));
        assert!(!doc.locked());
    }
}
