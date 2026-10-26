use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use toml_edit::{DocumentMut, Item, Value};

use crate::error::ApiError;
use crate::store;

/// User-added quick-access entry.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QuickPlace {
    pub name: String,
    pub path: String,
}

/// Program used to preview an extension FlowDir cannot render itself.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ExternalPreview {
    /// Lower-case extension without the dot, e.g. `docx`.
    pub ext: String,
    /// Command line; `{path}` is replaced with the selected file, appended when absent.
    pub command: String,
}

/// Rebound key combination; only ids the user changed are stored, defaults live in the UI.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ShortcutOverride {
    pub id: String,
    /// Canonical combination such as `ctrl+shift+f`.
    pub keys: String,
}

/// Parsed settings kept in memory so previews do not re-read the TOML on every selection.
#[derive(Default)]
pub struct SettingsCache(Mutex<Option<Arc<Settings>>>);

impl SettingsCache {
    pub fn get<R: Runtime>(&self, app: &AppHandle<R>) -> Arc<Settings> {
        let mut slot = self.0.lock().unwrap();
        slot.get_or_insert_with(|| Arc::new(load(app))).clone()
    }

    pub fn set(&self, settings: Settings) {
        *self.0.lock().unwrap() = Some(Arc::new(settings));
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub language: String,
    pub theme: String,
    /// Preview panel visibility on startup.
    pub preview_open: bool,
    /// New panes start with hidden/system entries visible.
    pub show_hidden: bool,
    /// Use Everything as the search provider when available.
    pub use_everything: bool,
    /// Sidebar (quick access + drives) visibility.
    pub sidebar_open: bool,
    /// Sidebar width in px while open; drag-resizable in the UI.
    pub sidebar_width: u32,
    /// Preview panel width in px while open; drag-resizable in the UI.
    pub preview_width: u32,
    /// Extra quick-access entries appended after the built-in ones.
    pub custom_places: Vec<QuickPlace>,
    /// Keys of built-in quick-access entries the user removed.
    pub hidden_places: Vec<String>,
    /// Per-extension preview programs for formats the built-in previewer cannot render.
    pub external_preview: Vec<ExternalPreview>,
    /// Rebound shortcuts; absent ids keep their defaults.
    pub shortcuts: Vec<ShortcutOverride>,
    /// WebView2 hardware compositing. Takes effect on the next launch because the
    /// argument is fixed when the browser process is created.
    pub gpu_accel: bool,
    /// Automatic update check interval in hours; `0` disables background checks and
    /// leaves only the manual button in Settings.
    pub update_interval_hours: u32,
    /// Single UI font family, assembled into a stack by the frontend.
    pub font_family: String,
    /// Base UI font size in px; drives the rem scale of the whole interface.
    pub font_size: u32,
}

fn default_language() -> String {
    "zh-CN".to_string()
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_font_family() -> String {
    "Segoe UI Variable Text".to_string()
}

fn default_font_size() -> u32 {
    13
}

const FONT_SIZE_MIN: u32 = 12;
const FONT_SIZE_MAX: u32 = 20;

/// A font family, not a stack: quotes, commas and separators would change the generated
/// CSS or let a hand-edited file break out of the property.
fn sanitize_font_family(value: &str) -> String {
    let trimmed = value.trim();
    let valid = !trimmed.is_empty()
        && trimmed.len() <= 64
        && !trimmed
            .chars()
            .any(|c| c.is_control() || matches!(c, ';' | '{' | '}' | '"' | '\'' | ',' | '\\'));
    if valid {
        trimmed.to_string()
    } else {
        default_font_family()
    }
}

fn sanitize_font_size(value: u32) -> u32 {
    if (FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&value) {
        value
    } else {
        default_font_size()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: default_language(),
            theme: default_theme(),
            preview_open: true,
            show_hidden: false,
            use_everything: true,
            sidebar_open: true,
            sidebar_width: 176,
            preview_width: 320,
            custom_places: Vec::new(),
            hidden_places: Vec::new(),
            external_preview: Vec::new(),
            shortcuts: Vec::new(),
            gpu_accel: true,
            update_interval_hours: 24,
            font_family: default_font_family(),
            font_size: default_font_size(),
        }
    }
}

impl Settings {
    /// Repairs values that a hand-edited file or an older build may carry.
    pub fn normalize(mut self) -> Self {
        self.font_family = sanitize_font_family(&self.font_family);
        self.font_size = sanitize_font_size(self.font_size);
        self
    }
}

const FILE: &str = "settings.toml";

/// Seed document for a fresh install. Comments written here survive later saves, which is
/// the whole reason this file is TOML and not JSON.
const TEMPLATE: &str = r#"# FlowDir 设置 / settings
# 改完立即生效，只有 gpu_accel 需要重启。 / Applies immediately; only gpu_accel needs a restart.

# 界面语言 / UI language: "zh-CN" | "en"
language = "zh-CN"

# 主题 / theme: "system" | "light" | "dark"
theme = "system"

# 界面字体与字号（像素，12–20）/ UI font family and size in px
font_family = "Segoe UI Variable Text"
font_size = 13

# 启动时展开预览面板 / show the preview panel on launch
preview_open = true

# 默认显示隐藏和系统项 / list hidden and system entries by default
show_hidden = false

# 搜索优先用 Everything（运行中才生效）/ prefer Everything while it is running
use_everything = true

# 显示左侧快速访问栏 / show the quick-access sidebar
sidebar_open = true

# 侧边栏和预览面板宽度（像素，可拖拽）/ sidebar and preview widths in px, drag-resizable
sidebar_width = 176
preview_width = 320

# WebView2 硬件合成，关闭省约 90MB 内存 / GPU compositing, saves ~90MB when off
gpu_accel = true

# 自动检查更新间隔（小时，0 = 只在设置里手动检查）
# automatic update check interval in hours; 0 = manual checks only
update_interval_hours = 24

# 额外的快速访问项 / extra quick-access entries
# custom_places = [{ name = "项目", path = 'D:\Codes' }]
custom_places = []

# 已删除的内置快速访问项 / removed built-ins: home | desktop | downloads | documents
hidden_places = []

# 外部预览程序：为内置预览不支持的扩展名指定程序，{path} 会替换为文件路径，
# 省略 {path} 时自动追加到命令末尾 / external preview per extension; {path} is
# replaced with the file path, or appended when missing
# external_preview = [{ ext = "docx", command = '"C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE" "{path}"' }]
external_preview = []

# 快捷键覆盖（只记录改过的项，默认值内置）/ shortcut overrides, defaults live in the app
# shortcuts = [{ id = "search", keys = "ctrl+shift+f" }]
shortcuts = []
"#;

/// On-disk shape: snake_case keys, no coupling to the camelCase IPC payload.
#[derive(Deserialize)]
#[serde(default)]
struct FileSettings {
    language: String,
    theme: String,
    preview_open: bool,
    show_hidden: bool,
    use_everything: bool,
    sidebar_open: bool,
    sidebar_width: u32,
    preview_width: u32,
    gpu_accel: bool,
    update_interval_hours: u32,
    font_family: String,
    font_size: u32,
    custom_places: Vec<QuickPlace>,
    hidden_places: Vec<String>,
    external_preview: Vec<ExternalPreview>,
    shortcuts: Vec<ShortcutOverride>,
}

impl Default for FileSettings {
    fn default() -> Self {
        let base = Settings::default();
        Self {
            language: base.language,
            theme: base.theme,
            preview_open: base.preview_open,
            show_hidden: base.show_hidden,
            use_everything: base.use_everything,
            sidebar_open: base.sidebar_open,
            sidebar_width: base.sidebar_width,
            preview_width: base.preview_width,
            gpu_accel: base.gpu_accel,
            update_interval_hours: base.update_interval_hours,
            font_family: base.font_family,
            font_size: base.font_size,
            custom_places: base.custom_places,
            hidden_places: base.hidden_places,
            external_preview: base.external_preview,
            shortcuts: base.shortcuts,
        }
    }
}

impl From<FileSettings> for Settings {
    fn from(file: FileSettings) -> Self {
        Self {
            language: file.language,
            theme: file.theme,
            preview_open: file.preview_open,
            show_hidden: file.show_hidden,
            use_everything: file.use_everything,
            sidebar_open: file.sidebar_open,
            sidebar_width: file.sidebar_width,
            preview_width: file.preview_width,
            gpu_accel: file.gpu_accel,
            update_interval_hours: file.update_interval_hours,
            font_family: file.font_family,
            font_size: file.font_size,
            custom_places: file.custom_places,
            hidden_places: file.hidden_places,
            external_preview: file.external_preview,
            shortcuts: file.shortcuts,
        }
    }
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Settings {
    let Ok(dir) = store::config_dir(app) else {
        return Settings::default();
    };
    let path = dir.join(FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        match toml_edit::de::from_str::<FileSettings>(&text) {
            Ok(file) => {
                return Settings::from(file).normalize();
            }
            Err(_) => {
                // Keep whatever the user had instead of letting the next save overwrite it.
                let _ = std::fs::copy(&path, dir.join("settings.rejected.toml"));
            }
        }
    }
    // One-time conversion from the JSON file used before TOML support.
    let legacy = dir.join("settings.json");
    if let Ok(bytes) = std::fs::read(&legacy) {
        if let Ok(settings) = serde_json::from_slice::<Settings>(&bytes) {
            let settings = settings.normalize();
            if save(app, &settings).is_ok() {
                let _ = std::fs::remove_file(&legacy);
            }
            return settings;
        }
    }
    // Materialize the commented template so the file is discoverable and editable.
    let defaults = Settings::default();
    let _ = save(app, &defaults);
    defaults
}

pub fn save<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<(), ApiError> {
    let dir = store::config_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new("CONFIG_DIR", e.to_string()))?;
    let path = dir.join(FILE);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| TEMPLATE.to_string());
    let mut doc = text
        .parse::<DocumentMut>()
        .unwrap_or_else(|_| TEMPLATE.parse().expect("template parses"));

    set_value(&mut doc, "language", Value::from(settings.language.as_str()));
    set_value(&mut doc, "theme", Value::from(settings.theme.as_str()));
    set_value(&mut doc, "preview_open", Value::from(settings.preview_open));
    set_value(&mut doc, "show_hidden", Value::from(settings.show_hidden));
    set_value(&mut doc, "use_everything", Value::from(settings.use_everything));
    set_value(&mut doc, "sidebar_open", Value::from(settings.sidebar_open));
    set_value(&mut doc, "sidebar_width", Value::from(settings.sidebar_width as i64));
    set_value(&mut doc, "preview_width", Value::from(settings.preview_width as i64));
    set_value(&mut doc, "gpu_accel", Value::from(settings.gpu_accel));
    set_value(
        &mut doc,
        "update_interval_hours",
        Value::from(settings.update_interval_hours as i64),
    );
    set_value(&mut doc, "font_family", Value::from(settings.font_family.as_str()));
    set_value(&mut doc, "font_size", Value::from(settings.font_size as i64));

    let mut hidden = toml_edit::Array::new();
    for key in &settings.hidden_places {
        hidden.push(key.as_str());
    }
    set_value(&mut doc, "hidden_places", Value::Array(hidden));

    let mut external = toml_edit::Array::new();
    for rule in &settings.external_preview {
        let mut entry = toml_edit::InlineTable::new();
        entry.insert("ext", Value::from(rule.ext.as_str()));
        entry.insert("command", Value::from(rule.command.as_str()));
        external.push(Value::InlineTable(entry));
    }
    set_value(&mut doc, "external_preview", Value::Array(external));

    let mut shortcuts = toml_edit::Array::new();
    for shortcut in &settings.shortcuts {
        let mut entry = toml_edit::InlineTable::new();
        entry.insert("id", Value::from(shortcut.id.as_str()));
        entry.insert("keys", Value::from(shortcut.keys.as_str()));
        shortcuts.push(Value::InlineTable(entry));
    }
    set_value(&mut doc, "shortcuts", Value::Array(shortcuts));

    // Written as an inline array, not `[[custom_places]]`: array-of-tables entries have no
    // stable position in the document, so rewriting them would drag the key to the end and
    // strand the comments that documented it. Reading still accepts both forms.
    let mut places = toml_edit::Array::new();
    for place in &settings.custom_places {
        let mut entry = toml_edit::InlineTable::new();
        entry.insert("name", Value::from(place.name.as_str()));
        entry.insert("path", Value::from(place.path.as_str()));
        places.push(Value::InlineTable(entry));
    }
    set_value(&mut doc, "custom_places", Value::Array(places));

    store::write_atomic(&path, doc.to_string().as_bytes())
        .map_err(|e| ApiError::new("IO", e.to_string()))
}

/// Replaces a key's value but keeps the surrounding decoration, so comments above the key
/// and inline trailing comments survive every save.
fn set_value(doc: &mut DocumentMut, key: &str, value: Value) {
    let table = doc.as_table_mut();
    let was_table_array = matches!(table.get(key), Some(Item::ArrayOfTables(_)));
    match table.get_mut(key) {
        Some(item) => {
            let decor = item.as_value().map(|old| old.decor().clone());
            *item = Item::Value(value);
            if let Some(new) = item.as_value_mut() {
                let mut kept = decor.unwrap_or_default();
                // Values created in code carry no leading space; keep `key = value` spacing
                // even when the previous item was an array-of-tables with no value decor.
                if kept.prefix().is_none() {
                    kept.set_prefix(" ");
                }
                *new.decor_mut() = kept;
            }
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
    // An `[[key]]` header has no `key = value` spacing; restore it when the entry becomes a
    // plain value, otherwise the line renders as `key= []`.
    if was_table_array {
        if let Some(mut k) = doc.as_table_mut().key_mut(key) {
            k.leaf_decor_mut().set_suffix(" ");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_font_values_fall_back() {
        let parsed = Settings {
            font_family: "  Segoe UI  ".to_string(),
            font_size: 18,
            ..Settings::default()
        }
        .normalize();
        assert_eq!(parsed.font_family, "Segoe UI");
        assert_eq!(parsed.font_size, 18);

        for bad in ["", "Bad; color: red", "\"Arial\", serif", "Arial}"] {
            let parsed = Settings {
                font_family: bad.to_string(),
                ..Settings::default()
            }
            .normalize();
            assert_eq!(parsed.font_family, "Segoe UI Variable Text", "{bad}");
        }

        for bad in [0, 11, 21, u32::MAX] {
            let parsed = Settings {
                font_size: bad,
                ..Settings::default()
            }
            .normalize();
            assert_eq!(parsed.font_size, 13, "{bad}");
        }
    }

    #[test]
    fn template_and_inline_rules_parse() {
        let seed: FileSettings = toml_edit::de::from_str(TEMPLATE).unwrap();
        assert!(seed.external_preview.is_empty());

        let text = r#"external_preview = [{ ext = "docx", command = '"C:\Tools\WINWORD.EXE" "{path}"' }]"#;
        let parsed: FileSettings = toml_edit::de::from_str(text).unwrap();
        assert_eq!(parsed.external_preview.len(), 1);
        assert_eq!(parsed.external_preview[0].ext, "docx");
        assert_eq!(
            parsed.external_preview[0].command,
            r#""C:\Tools\WINWORD.EXE" "{path}""#
        );

        let text = r#"shortcuts = [{ id = "search", keys = "ctrl+shift+f" }]"#;
        let parsed: FileSettings = toml_edit::de::from_str(text).unwrap();
        assert_eq!(parsed.shortcuts.len(), 1);
        assert_eq!(parsed.shortcuts[0].id, "search");
        assert_eq!(parsed.shortcuts[0].keys, "ctrl+shift+f");
    }
}
