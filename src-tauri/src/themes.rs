//! Theme packages: discovery, validation, import/export, editing, and the
//! sandboxed frame document each theme runs in.
//!
//! Every theme *extends Default*: tokens, layout and components it leaves out
//! are taken from the Default theme, so a theme can be a single theme.json or a
//! complete UI replacement.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::jsonschema_lite;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

pub const DEFAULT_ID: &str = "default";
pub const LIVE_ID: &str = "live";
const MAX_THEME_BYTES: u64 = 20 * 1024 * 1024;
const MAX_FILES: usize = 400;

pub fn theme_schema() -> Value {
    serde_json::from_str(include_str!("../../themes/theme.schema.json")).unwrap()
}
pub fn layout_schema() -> Value {
    serde_json::from_str(include_str!("../../themes/layout.schema.json")).unwrap()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    pub builtin: bool,
    pub has_script: bool,
    pub error: Option<String>,
    pub accent: String,
    pub bg: String,
}

/// A fully resolved theme (merged over Default) ready to render.
#[derive(Clone, Debug)]
pub struct LoadedTheme {
    pub id: String,
    pub meta: Value,
    pub layout: Value,
    pub components: BTreeMap<String, String>,
    pub has_css: bool,
    pub has_script: bool,
}

/// Where a theme's files come from.
enum Source {
    Embedded,
    Dir(PathBuf),
}

pub struct ThemeManager {
    dir: PathBuf,
    dev_root: Option<PathBuf>,
    pub live_dir: std::sync::RwLock<Option<PathBuf>>,
}

/// Allowed file types inside a theme package.
pub fn allowed_rel_path(rel: &str) -> bool {
    if rel.contains("..") || rel.starts_with('/') || rel.contains('\\') || rel.contains(':') || rel.len() > 160 {
        return false;
    }
    match rel {
        "theme.json" | "layout.json" | "theme.css" | "script.js" | "README.md" => true,
        _ => {
            if let Some(name) = rel.strip_prefix("components/") {
                return name.ends_with(".html")
                    && !name.contains('/')
                    && name[..name.len() - 5].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            }
            if let Some(a) = rel.strip_prefix("assets/") {
                let ext = a.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
                return a.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
                    && ["png", "jpg", "jpeg", "webp", "gif", "svg", "woff2", "woff", "ttf", "otf"].contains(&ext.as_str());
            }
            false
        }
    }
}

pub fn mime_for(rel: &str) -> &'static str {
    match rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "html" => "text/html; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "md" => "text/markdown; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Reject component templates that try to escape the template sandbox.
pub fn check_component(name: &str, html: &str) -> Result<()> {
    let l = html.to_ascii_lowercase();
    for bad in ["<script", "</template", "<iframe", "<object", "<embed", "<base", "<meta", "<link", "javascript:", "srcdoc"] {
        if l.contains(bad) {
            bail!("components/{name}.html: `{bad}` is not allowed in templates (use script.js)");
        }
    }
    // Inline event handlers (onclick= etc.) are blocked by CSP anyway; reject for clarity.
    let re = regex::Regex::new(r"(?i)\son[a-z]+\s*=").unwrap();
    if re.is_match(html) {
        bail!("components/{name}.html: inline event handlers are not allowed (use data-action or script.js)");
    }
    if html.len() > 256 * 1024 {
        bail!("components/{name}.html: too large");
    }
    Ok(())
}

fn merge(a: &mut Value, b: &Value) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (a, b) => *a = b.clone(),
    }
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let s: String = s.chars().take(40).collect();
    if s.is_empty() || s == DEFAULT_ID || s == LIVE_ID { format!("theme-{s}") } else { s }
}

impl ThemeManager {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        // Debug builds read Default + runtime straight from the repo so UI edits
        // show up on reload without recompiling.
        let dev_root = if cfg!(debug_assertions) {
            let p = Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(|p| p.to_path_buf());
            p.filter(|p| p.join("themes/default/theme.json").exists())
        } else {
            None
        };
        Self { dir, dev_root, live_dir: std::sync::RwLock::new(None) }
    }

    fn source(&self, id: &str) -> Result<Source> {
        if id == DEFAULT_ID {
            return Ok(match &self.dev_root {
                Some(r) => Source::Dir(r.join("themes/default")),
                None => Source::Embedded,
            });
        }
        if id == LIVE_ID {
            let d = self.live_dir.read().unwrap().clone().ok_or_else(|| anyhow!("live link is off"))?;
            return Ok(Source::Dir(d));
        }
        if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') || id.is_empty() || id.len() > 64 {
            bail!("bad theme id");
        }
        let d = self.dir.join(id);
        if !d.join("theme.json").exists() {
            bail!("theme `{id}` is not installed");
        }
        Ok(Source::Dir(d))
    }

    /// Read one file of a theme (None if absent). Path must be allowed.
    pub fn file(&self, id: &str, rel: &str) -> Result<Option<Vec<u8>>> {
        if !allowed_rel_path(rel) {
            bail!("path not allowed");
        }
        Ok(match self.source(id)? {
            Source::Embedded => embedded::DEFAULT_THEME.iter().find(|(p, _)| *p == rel).map(|(_, b)| b.to_vec()),
            Source::Dir(d) => std::fs::read(d.join(rel)).ok(),
        })
    }

    pub fn runtime_file(&self, rel: &str) -> Option<Vec<u8>> {
        if rel.contains("..") || rel.contains('\\') {
            return None;
        }
        if let Some(r) = &self.dev_root {
            return std::fs::read(r.join("ui/runtime").join(rel)).ok();
        }
        embedded::RUNTIME.iter().find(|(p, _)| *p == rel).map(|(_, b)| b.to_vec())
    }

    fn list_files(&self, id: &str) -> Result<Vec<String>> {
        Ok(match self.source(id)? {
            Source::Embedded => embedded::DEFAULT_THEME.iter().map(|(p, _)| p.to_string()).collect(),
            Source::Dir(d) => {
                let mut v = Vec::new();
                walk(&d, &d, &mut v);
                v.into_iter().filter(|p| allowed_rel_path(p)).collect()
            }
        })
    }

    fn json_file(&self, id: &str, rel: &str) -> Result<Option<Value>> {
        match self.file(id, rel)? {
            None => Ok(None),
            Some(b) => {
                let v: Value = serde_json::from_slice(&b).with_context(|| format!("{rel} is not valid JSON"))?;
                Ok(Some(v))
            }
        }
    }

    fn components(&self, id: &str) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for rel in self.list_files(id)? {
            if let Some(name) = rel.strip_prefix("components/").and_then(|n| n.strip_suffix(".html")) {
                let b = self.file(id, &rel)?.unwrap_or_default();
                let html = String::from_utf8(b).map_err(|_| anyhow!("{rel} is not UTF-8"))?;
                check_component(name, &html)?;
                out.insert(name.to_string(), html);
            }
        }
        Ok(out)
    }

    /// Validate a theme's own files (not merged). Returns its theme.json.
    pub fn validate(&self, id: &str) -> Result<Value> {
        let meta = self.json_file(id, "theme.json")?.ok_or_else(|| anyhow!("theme.json is missing"))?;
        jsonschema_lite::validate(&theme_schema(), &meta).map_err(|e| anyhow!("theme.json: {}", e.join("; ")))?;
        if let Some(layout) = self.json_file(id, "layout.json")? {
            jsonschema_lite::validate(&layout_schema(), &layout).map_err(|e| anyhow!("layout.json: {}", e.join("; ")))?;
            for mode in ["normal", "compact"] {
                if let Some(g) = layout.get(mode) {
                    check_grid(g).map_err(|e| anyhow!("layout.json {mode}: {e}"))?;
                }
            }
        }
        self.components(id)?;
        if let Some(css) = self.file(id, "theme.css")? {
            if css.len() > 512 * 1024 {
                bail!("theme.css is larger than 512 KB");
            }
            std::str::from_utf8(&css).map_err(|_| anyhow!("theme.css is not UTF-8"))?;
        }
        if let Some(js) = self.file(id, "script.js")? {
            if js.len() > 512 * 1024 {
                bail!("script.js is larger than 512 KB");
            }
        }
        Ok(meta)
    }

    /// Load a theme merged over Default.
    pub fn load(&self, id: &str) -> Result<LoadedTheme> {
        let base_meta = self.json_file(DEFAULT_ID, "theme.json")?.unwrap_or(Value::Null);
        let base_layout = self.json_file(DEFAULT_ID, "layout.json")?.unwrap_or(Value::Null);
        let mut components = self.components(DEFAULT_ID)?;
        if id == DEFAULT_ID {
            return Ok(LoadedTheme {
                id: id.into(),
                meta: base_meta,
                layout: base_layout,
                components,
                has_css: self.file(id, "theme.css")?.is_some(),
                has_script: self.file(id, "script.js")?.is_some(),
            });
        }
        let own = self.validate(id)?;
        let mut meta = base_meta;
        merge(&mut meta, &own);
        let mut layout = base_layout;
        if let Some(l) = self.json_file(id, "layout.json")? {
            merge(&mut layout, &l);
        }
        components.extend(self.components(id)?);
        Ok(LoadedTheme {
            id: id.into(),
            meta,
            layout,
            components,
            has_css: self.file(id, "theme.css")?.is_some(),
            has_script: self.file(id, "script.js")?.is_some(),
        })
    }

    pub fn list(&self) -> Vec<ThemeInfo> {
        let mut ids = vec![DEFAULT_ID.to_string()];
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            let mut v: Vec<String> = rd
                .flatten()
                .filter(|e| e.path().join("theme.json").exists())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            v.sort();
            ids.extend(v);
        }
        if self.live_dir.read().unwrap().is_some() {
            ids.push(LIVE_ID.into());
        }
        ids.into_iter()
            .map(|id| {
                let (meta, error) = match if id == DEFAULT_ID { self.json_file(&id, "theme.json").map(|m| m.unwrap_or_default()) } else { self.validate(&id) } {
                    Ok(m) => (m, None),
                    Err(e) => (self.json_file(&id, "theme.json").ok().flatten().unwrap_or_default(), Some(format!("{e:#}"))),
                };
                let g = |k: &str| meta.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
                ThemeInfo {
                    builtin: id == DEFAULT_ID,
                    has_script: self.file(&id, "script.js").ok().flatten().is_some(),
                    name: if g("name").is_empty() { id.clone() } else { g("name") },
                    author: g("author"),
                    version: g("version"),
                    description: g("description"),
                    accent: meta["colors"]["accent"].as_str().unwrap_or("#8b5cf6").to_string(),
                    bg: meta["colors"]["bg"].as_str().unwrap_or("#0e0e10").to_string(),
                    error,
                    id,
                }
            })
            .collect()
    }

    fn unique_id(&self, base: &str) -> String {
        let base = slug(base);
        let mut id = base.clone();
        let mut n = 2;
        while self.dir.join(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Import a `.theme` zip. Returns (id, has_script). Invalid themes are not installed.
    pub fn import_zip(&self, path: &Path) -> Result<(String, bool)> {
        let f = std::fs::File::open(path).context("open theme file")?;
        if f.metadata()?.len() > MAX_THEME_BYTES {
            bail!("theme file is larger than 20 MB");
        }
        let mut zip = zip::ZipArchive::new(f).context("not a valid .theme (zip) file")?;
        if zip.len() > MAX_FILES {
            bail!("too many files in theme");
        }
        // Allow a single top-level folder wrapping the files.
        let mut names: Vec<String> = (0..zip.len()).filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string())).collect();
        names.retain(|n| !n.ends_with('/'));
        let prefix = if names.iter().any(|n| n == "theme.json") {
            String::new()
        } else {
            let p = names.iter().find(|n| n.ends_with("/theme.json")).map(|n| n.trim_end_matches("theme.json").to_string());
            p.ok_or_else(|| anyhow!("theme.json is missing"))?
        };
        let staging = self.dir.join(format!(".import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let res = (|| -> Result<(String, bool)> {
            let mut total = 0u64;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i)?;
                if entry.is_dir() {
                    continue;
                }
                let name = entry.name().to_string();
                let Some(rel) = name.strip_prefix(&prefix) else { continue };
                if !allowed_rel_path(rel) {
                    bail!("file not allowed in a theme: {rel}");
                }
                total += entry.size();
                if total > MAX_THEME_BYTES * 3 {
                    bail!("theme expands to more than 60 MB");
                }
                let dest = staging.join(rel);
                std::fs::create_dir_all(dest.parent().unwrap())?;
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.by_ref().take(MAX_THEME_BYTES).read_to_end(&mut buf)?;
                std::fs::write(dest, buf)?;
            }
            // Validate in place using a temporary id.
            let tmp_id = staging.file_name().unwrap().to_string_lossy().to_string();
            let meta = self.validate_dir(&staging)?;
            let _ = tmp_id;
            let id = self.unique_id(meta["name"].as_str().unwrap_or("theme"));
            std::fs::rename(&staging, self.dir.join(&id))?;
            let has_script = self.dir.join(&id).join("script.js").exists();
            Ok((id, has_script))
        })();
        if res.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        res
    }

    fn validate_dir(&self, dir: &Path) -> Result<Value> {
        // Temporarily expose the staging dir as a theme id for validation.
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let id = name.trim_start_matches('.').to_string();
        let tmp = self.dir.join(&id);
        std::fs::rename(dir, &tmp)?;
        let r = self.validate(&id);
        std::fs::rename(&tmp, dir)?;
        r
    }

    pub fn export_zip(&self, id: &str, dest: &Path) -> Result<()> {
        let f = std::fs::File::create(dest)?;
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for rel in self.list_files(id)? {
            if let Some(b) = self.file(id, &rel)? {
                zw.start_file(rel, opts)?;
                zw.write_all(&b)?;
            }
        }
        zw.finish()?;
        Ok(())
    }

    pub fn duplicate(&self, id: &str) -> Result<String> {
        let meta = self.json_file(id, "theme.json")?.ok_or_else(|| anyhow!("theme.json missing"))?;
        let name = format!("{} copy", meta["name"].as_str().unwrap_or(id));
        let new_id = self.unique_id(&name);
        let dest = self.dir.join(&new_id);
        std::fs::create_dir_all(&dest)?;
        for rel in self.list_files(id)? {
            if let Some(mut b) = self.file(id, &rel)? {
                if rel == "theme.json" {
                    let mut m: Value = serde_json::from_slice(&b)?;
                    m["name"] = json!(name.chars().take(60).collect::<String>());
                    if m.get("author").map(|a| a == "mp3palace").unwrap_or(false) {
                        m["author"] = json!("me");
                    }
                    b = serde_json::to_vec_pretty(&m)?;
                }
                let p = dest.join(&rel);
                std::fs::create_dir_all(p.parent().unwrap())?;
                std::fs::write(p, b)?;
            }
        }
        Ok(new_id)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if id == DEFAULT_ID {
            bail!("the Default theme can't be deleted");
        }
        if id == LIVE_ID {
            bail!("unlink the live folder instead");
        }
        match self.source(id)? {
            Source::Dir(d) => std::fs::remove_dir_all(d)?,
            Source::Embedded => bail!("built-in"),
        }
        Ok(())
    }

    /// Text files for the editor.
    pub fn read_text_files(&self, id: &str) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for rel in self.list_files(id)? {
            if rel.ends_with(".json") || rel.ends_with(".css") || rel.ends_with(".js") || rel.ends_with(".html") {
                if let Some(b) = self.file(id, &rel)? {
                    out.insert(rel, String::from_utf8_lossy(&b).to_string());
                }
            }
        }
        Ok(out)
    }

    /// Save edited files. Validates the result; rolls back on failure.
    pub fn save(&self, id: &str, files: &BTreeMap<String, String>) -> Result<()> {
        if id == DEFAULT_ID {
            bail!("Default is read-only — duplicate it first");
        }
        let dir = match self.source(id)? {
            Source::Dir(d) => d,
            Source::Embedded => bail!("read-only"),
        };
        let mut backup = Vec::new();
        for (rel, body) in files {
            if !allowed_rel_path(rel) || rel.starts_with("assets/") {
                bail!("cannot write {rel}");
            }
            let p = dir.join(rel);
            backup.push((p.clone(), std::fs::read(&p).ok()));
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(&p, body)?;
        }
        if let Err(e) = self.validate(id) {
            for (p, old) in backup {
                match old {
                    Some(b) => {
                        let _ = std::fs::write(&p, b);
                    }
                    None => {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
            return Err(e);
        }
        Ok(())
    }

    /// The sandboxed document for a theme.
    pub fn frame_html(&self, t: &LoadedTheme, boot: &Value) -> String {
        let m = &t.meta;
        let c = &m["colors"];
        let f = &m["fonts"];
        let num = |k: &str, d: f64| m.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
        let col = |k: &str| c.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let density_pad = match m["density"].as_str() {
            Some("compact") => 0.75,
            Some("spacious") => 1.25,
            _ => 1.0,
        };
        let mut tokens = String::from(":root{");
        for (k, css) in [
            ("bg", "--bg"),
            ("surface", "--surface"),
            ("surface2", "--surface-2"),
            ("elevated", "--elevated"),
            ("text", "--text"),
            ("textMuted", "--text-muted"),
            ("accent", "--accent"),
            ("accentText", "--accent-text"),
            ("border", "--border"),
            ("hover", "--hover"),
            ("danger", "--danger"),
            ("scrollbar", "--scrollbar"),
        ] {
            let v = col(k);
            if !v.is_empty() {
                tokens.push_str(&format!("{css}:{v};"));
            }
        }
        tokens.push_str(&format!("--theme-accent:{};", col("accent")));
        for (k, css) in [("body", "--font-body"), ("heading", "--font-heading"), ("mono", "--font-mono")] {
            if let Some(v) = f.get(k).and_then(|v| v.as_str()) {
                tokens.push_str(&format!("{css}:{v};"));
            }
        }
        tokens.push_str(&format!(
            "--font-size:{}px;--radius:{}px;--density:{};--sidebar-w:{}px;--panel-w:{}px;--player-h:{}px;}}",
            f.get("size").and_then(|v| v.as_f64()).unwrap_or(14.0),
            num("radius", 8.0),
            density_pad,
            num("sidebarWidth", 240.0),
            num("panelWidth", 320.0),
            num("playerHeight", 84.0)
        ));
        let mut faces = String::new();
        if let Some(arr) = m.get("fontFaces").and_then(|v| v.as_array()) {
            for ff in arr {
                let fam = ff["family"].as_str().unwrap_or_default();
                let src = ff["src"].as_str().unwrap_or_default();
                if fam.is_empty() || !allowed_rel_path(src) {
                    continue;
                }
                faces.push_str(&format!(
                    "@font-face{{font-family:\"{fam}\";src:url(\"/t/{}/{src}\");font-weight:{};font-style:{};font-display:swap}}",
                    t.id,
                    ff["weight"].as_str().unwrap_or("400"),
                    ff["style"].as_str().unwrap_or("normal")
                ));
            }
        }
        // Component sources travel as raw strings inside the (non-executed) boot
        // JSON, never through the HTML parser, so {{…}} inside tags stays intact.
        let mut boot = boot.clone();
        boot["components"] = serde_json::to_value(&t.components).unwrap_or_default();
        let boot_json = serde_json::to_string(&boot).unwrap_or_default().replace("</", "<\\/").replace("<!--", "<\\!--");
        let theme_css = if t.has_css { format!("<link rel=\"stylesheet\" href=\"/t/{}/theme.css\">", t.id) } else { String::new() };
        // Custom themes get Default's stylesheet underneath their own.
        let base_theme_css = if t.id != DEFAULT_ID {
            format!("<link rel=\"stylesheet\" href=\"/t/{DEFAULT_ID}/theme.css\">")
        } else {
            String::new()
        };
        let script = if t.has_script { format!("<script src=\"/t/{}/script.js\"></script>", t.id) } else { String::new() };
        format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<link rel=\"stylesheet\" href=\"/rt/base.css\">{base_theme_css}\
<style id=\"mp3-tokens\">{tokens}{faces}</style>{theme_css}\
<script type=\"application/json\" id=\"mp3-boot\">{boot_json}</script>\
</head><body>\n<div id=\"mp3-root\"></div>\
<script src=\"/rt/runtime.js\"></script>{script}</body></html>"
        )
    }
}

fn check_grid(g: &Value) -> Result<()> {
    let areas: Vec<Vec<&str>> =
        g["areas"].as_array().unwrap().iter().map(|r| r.as_str().unwrap_or("").split_whitespace().collect()).collect();
    let cols = g["columns"].as_array().map(|c| c.len()).unwrap_or(0);
    let rows = g["rows"].as_array().map(|r| r.len()).unwrap_or(0);
    if areas.len() != rows {
        bail!("{} area rows but {} row sizes", areas.len(), rows);
    }
    for r in &areas {
        if r.len() != cols {
            bail!("each area row needs {cols} cells");
        }
    }
    if !areas.iter().flatten().any(|a| *a == "main") {
        bail!("the main region must be placed");
    }
    Ok(())
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[allow(dead_code)]
fn _unused(_: Map<String, Value>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("mp3p-themes-{}-{}", std::process::id(), fastrand_u()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn fastrand_u() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    fn make_zip(path: &Path, files: &[(&str, &str)]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        for (n, b) in files {
            z.start_file(*n, o).unwrap();
            z.write_all(b.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn default_loads_and_renders() {
        let m = ThemeManager::new(tmp());
        let t = m.load(DEFAULT_ID).unwrap();
        assert!(t.components.contains_key("player-bar"));
        let html = m.frame_html(&t, &json!({"x": "</script>"}));
        assert!(html.contains("\"track-row\":"));
        assert!(!html.contains("\"x\":\"</script>\""));
    }

    #[test]
    fn import_validate_duplicate_export_delete() {
        let dir = tmp();
        let m = ThemeManager::new(dir.clone());
        let z = dir.join("ok.theme");
        std::fs::create_dir_all(&dir).unwrap();
        make_zip(&z, &[
            ("theme.json", r##"{"name":"Neon Test","version":"1.0","apiVersion":1,"colors":{"accent":"#ff00ff"}}"##),
            ("theme.css", ".mp3-player{border-top:1px solid var(--accent)}"),
            ("components/track-row.html", "<div class=\"mp3-track-row\" data-action=\"play\">{{track.title}}</div>"),
            ("script.js", "mp3.on('trackChanged', () => {});"),
        ]);
        let (id, has_script) = m.import_zip(&z).unwrap();
        assert_eq!(id, "neon-test");
        assert!(has_script);
        let t = m.load(&id).unwrap();
        assert_eq!(t.meta["colors"]["accent"], "#ff00ff");
        assert!(t.meta["colors"]["bg"].is_string(), "inherits Default tokens");
        assert!(t.components["track-row"].contains("{{track.title}}"));
        assert!(t.components.contains_key("player-bar"), "inherits Default components");

        let dup = m.duplicate(&id).unwrap();
        assert_eq!(dup, "neon-test-copy");
        let out = dir.join("export.theme");
        m.export_zip(&dup, &out).unwrap();
        let (again, _) = m.import_zip(&out).unwrap();
        assert_eq!(again, "neon-test-copy-2");
        m.delete(&dup).unwrap();
        assert!(m.delete(DEFAULT_ID).is_err());
        assert!(m.list().iter().any(|t| t.id == "neon-test"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_bad_packages() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let m = ThemeManager::new(dir.clone());
        let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("no theme.json", vec![("theme.css", "")]),
            ("bad json", vec![("theme.json", "{")]),
            ("schema", vec![("theme.json", r#"{"name":"x","version":"1","apiVersion":9}"#)]),
            ("exe", vec![("theme.json", r#"{"name":"x","version":"1","apiVersion":1}"#), ("assets/x.exe", "MZ")]),
            ("traversal", vec![("theme.json", r#"{"name":"x","version":"1","apiVersion":1}"#), ("../evil.css", "")]),
            ("script in template", vec![("theme.json", r#"{"name":"x","version":"1","apiVersion":1}"#), ("components/x.html", "<script>alert(1)</script>")]),
            ("handler", vec![("theme.json", r#"{"name":"x","version":"1","apiVersion":1}"#), ("components/x.html", "<img src=x onerror=alert(1)>")]),
            ("layout", vec![("theme.json", r#"{"name":"x","version":"1","apiVersion":1}"#), ("layout.json", r#"{"normal":{"areas":["sidebar player"],"columns":["1fr","1fr"],"rows":["1fr"]}}"#)]),
        ];
        for (name, files) in cases {
            let z = dir.join(format!("{}.theme", name.replace(' ', "_")));
            make_zip(&z, &files);
            assert!(m.import_zip(&z).is_err(), "{name} should be rejected");
        }
        assert_eq!(m.list().len(), 1, "nothing installed");
        let _ = std::fs::remove_dir_all(dir);
    }
}
