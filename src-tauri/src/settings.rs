use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use toml_edit::{DocumentMut, Item, Value};

use crate::error::ApiError;
use crate::store;

/// Set when `settings.toml` could not be parsed and the user is running on the fallback
/// chain. The frontend asks once and shows a single toast.
pub static CONFIG_REJECTED: AtomicBool = AtomicBool::new(false);

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
    /// System notification when a long operation finishes in the background.
    pub notify_ops: bool,
    /// Diagnostic log verbosity: error | warn | info | debug.
    pub log_level: String,
    /// Single UI font family, assembled into a stack by the frontend.
    pub font_family: String,
    /// Base UI font size in px; drives the rem scale of the whole interface.
    pub font_size: u32,
    /// Tab-strip font family; empty inherits `font_family`.
    pub tab_font_family: String,
    /// Tab-strip font size in px; drives the strip height and tab icons.
    pub tab_font_size: u32,
    /// File-list font family; empty inherits `font_family`.
    pub list_font_family: String,
    /// File-list font size in px; drives rows, columns and icons.
    pub list_font_size: u32,
    /// Navigation-bar font family; empty inherits `font_family`.
    pub nav_font_family: String,
    /// Navigation-bar font size in px; drives its controls.
    pub nav_font_size: u32,
    /// File-list size column width in px at the default font size; drag-resizable.
    pub list_size_width: u32,
    /// File-list modified column width in px at the default font size; drag-resizable.
    pub list_time_width: u32,
    /// Drive overview capacity column width in px at the default font size; drag-resizable.
    pub drive_capacity_width: u32,
    /// Drive overview free-space column width in px at the default font size; drag-resizable.
    pub drive_free_width: u32,
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

fn default_tab_font_size() -> u32 {
    12
}

fn default_list_font_size() -> u32 {
    13
}

fn default_nav_font_size() -> u32 {
    12
}

fn default_list_size_width() -> u32 {
    96
}

fn default_list_time_width() -> u32 {
    142
}

fn default_drive_capacity_width() -> u32 {
    150
}

fn default_drive_free_width() -> u32 {
    84
}

const FONT_SIZE_MIN: u32 = 12;
const FONT_SIZE_MAX: u32 = 20;
/// Region sizes may go below the global floor: compact tab strips stay usable.
const REGION_FONT_SIZE_MIN: u32 = 10;

/// Column widths are stored unscaled; the frontend multiplies them by the font scale.
/// The lower bounds keep the header labels and the longest "[123,456 items]" readable.
const LIST_COL_MIN: u32 = 56;
const LIST_TIME_MIN: u32 = 84;
/// "123.4 GB / 465.7 GB" plus the usage bar needs more room than a plain file size.
const DRIVE_CAPACITY_MIN: u32 = 120;
const DRIVE_FREE_MIN: u32 = 56;
const LIST_COL_MAX: u32 = 480;

/// A font family, not a stack: quotes, commas and separators would change the generated
/// CSS or let a hand-edited file break out of the property.
fn valid_font_family(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, ';' | '{' | '}' | '"' | '\'' | ',' | '\\'))
}

fn sanitize_font_family(value: &str) -> String {
    let trimmed = value.trim();
    if valid_font_family(trimmed) {
        trimmed.to_string()
    } else {
        default_font_family()
    }
}

/// Region families may be empty, which means "inherit the UI font"; invalid input falls
/// back to inherit rather than silently switching to a different installed face.
fn sanitize_region_font_family(value: &str) -> String {
    let trimmed = value.trim();
    if valid_font_family(trimmed) {
        trimmed.to_string()
    } else {
        String::new()
    }
}

fn sanitize_font_size(value: u32) -> u32 {
    if (FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&value) {
        value
    } else {
        default_font_size()
    }
}

fn sanitize_region_font_size(value: u32, fallback: u32) -> u32 {
    if (REGION_FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&value) {
        value
    } else {
        fallback
    }
}

fn sanitize_list_col(value: u32, min: u32, fallback: u32) -> u32 {
    if (min..=LIST_COL_MAX).contains(&value) {
        value
    } else {
        fallback
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
            notify_ops: true,
            log_level: "info".to_string(),
            font_family: default_font_family(),
            font_size: default_font_size(),
            tab_font_family: String::new(),
            tab_font_size: default_tab_font_size(),
            list_font_family: String::new(),
            list_font_size: default_list_font_size(),
            nav_font_family: String::new(),
            nav_font_size: default_nav_font_size(),
            list_size_width: default_list_size_width(),
            list_time_width: default_list_time_width(),
            drive_capacity_width: default_drive_capacity_width(),
            drive_free_width: default_drive_free_width(),
        }
    }
}

impl Settings {
    /// Repairs values that a hand-edited file or an older build may carry.
    pub fn normalize(mut self) -> Self {
        self.language = match self.language.as_str() {
            "en" => "en".to_string(),
            _ => "zh-CN".to_string(),
        };
        // Keep in sync with `src/themes.ts`; an unknown id falls back to the OS scheme.
        self.theme = match self.theme.as_str() {
            "light" => "light".to_string(),
            "sepia" => "sepia".to_string(),
            "mist" => "mist".to_string(),
            "dark" => "dark".to_string(),
            "midnight" => "midnight".to_string(),
            "graphite" => "graphite".to_string(),
            "nord" => "nord".to_string(),
            "forest" => "forest".to_string(),
            _ => "system".to_string(),
        };
        // The frontend enforces the same minimum; the upper bound only rejects absurd
        // hand-edited values that would push the panes off screen.
        self.sidebar_width = self.sidebar_width.clamp(140, 2000);
        self.preview_width = self.preview_width.clamp(220, 2000);
        self.update_interval_hours = match self.update_interval_hours {
            0 | 6 | 12 | 24 | 72 => self.update_interval_hours,
            _ => 24,
        };
        self.log_level = match self.log_level.as_str() {
            "error" | "warn" | "info" | "debug" => self.log_level,
            _ => "info".to_string(),
        };
        self.font_family = sanitize_font_family(&self.font_family);
        self.font_size = sanitize_font_size(self.font_size);
        self.tab_font_family = sanitize_region_font_family(&self.tab_font_family);
        self.tab_font_size = sanitize_region_font_size(self.tab_font_size, default_tab_font_size());
        self.list_font_family = sanitize_region_font_family(&self.list_font_family);
        self.list_font_size = sanitize_region_font_size(self.list_font_size, default_list_font_size());
        self.nav_font_family = sanitize_region_font_family(&self.nav_font_family);
        self.nav_font_size = sanitize_region_font_size(self.nav_font_size, default_nav_font_size());
        self.list_size_width = sanitize_list_col(
            self.list_size_width,
            LIST_COL_MIN,
            default_list_size_width(),
        );
        self.list_time_width = sanitize_list_col(
            self.list_time_width,
            LIST_TIME_MIN,
            default_list_time_width(),
        );
        self.drive_capacity_width = sanitize_list_col(
            self.drive_capacity_width,
            DRIVE_CAPACITY_MIN,
            default_drive_capacity_width(),
        );
        self.drive_free_width =
            sanitize_list_col(self.drive_free_width, DRIVE_FREE_MIN, default_drive_free_width());
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

# 主题 / theme: "system" | "light" | "sepia" | "mist" | "dark" | "midnight" | "graphite" | "nord" | "forest"
theme = "system"

# 界面字体与字号（像素，12–20）/ UI font family and size in px
font_family = "Segoe UI Variable Text"
font_size = 13

# 标签页、文件列表、导航栏的字体（留空跟随界面字体）与字号（10–20）/ per-region font and size
tab_font_family = ""
tab_font_size = 12
list_font_family = ""
list_font_size = 13
nav_font_family = ""
nav_font_size = 12

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

# 文件列表“大小/修改时间”列宽（像素，拖表头分隔线调整，双击恢复默认）
# file-list size and modified column widths in px; drag the header divider, double-click to reset
list_size_width = 96
list_time_width = 142

# 此电脑“容量/剩余”列宽（像素，可拖拽，双击恢复默认）
# drive overview capacity and free-space column widths in px; drag or double-click to reset
drive_capacity_width = 150
drive_free_width = 84

# WebView2 硬件合成，关闭省约 90MB 内存 / GPU compositing, saves ~90MB when off
gpu_accel = true

# 自动检查更新间隔（小时，0 = 只在设置里手动检查）
# automatic update check interval in hours; 0 = manual checks only
update_interval_hours = 24

# 长任务在后台完成时发系统通知 / system notification when a long operation finishes
notify_ops = true

# 诊断日志级别 / diagnostic log level: "error" | "warn" | "info" | "debug"
log_level = "info"

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
    notify_ops: bool,
    log_level: String,
    font_family: String,
    font_size: u32,
    tab_font_family: String,
    tab_font_size: u32,
    list_font_family: String,
    list_font_size: u32,
    nav_font_family: String,
    nav_font_size: u32,
    list_size_width: u32,
    list_time_width: u32,
    drive_capacity_width: u32,
    drive_free_width: u32,
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
            notify_ops: base.notify_ops,
            log_level: base.log_level,
            font_family: base.font_family,
            font_size: base.font_size,
            tab_font_family: base.tab_font_family,
            tab_font_size: base.tab_font_size,
            list_font_family: base.list_font_family,
            list_font_size: base.list_font_size,
            nav_font_family: base.nav_font_family,
            nav_font_size: base.nav_font_size,
            list_size_width: base.list_size_width,
            list_time_width: base.list_time_width,
            drive_capacity_width: base.drive_capacity_width,
            drive_free_width: base.drive_free_width,
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
            notify_ops: file.notify_ops,
            log_level: file.log_level,
            font_family: file.font_family,
            font_size: file.font_size,
            tab_font_family: file.tab_font_family,
            tab_font_size: file.tab_font_size,
            list_font_family: file.list_font_family,
            list_font_size: file.list_font_size,
            nav_font_family: file.nav_font_family,
            nav_font_size: file.nav_font_size,
            list_size_width: file.list_size_width,
            list_time_width: file.list_time_width,
            drive_capacity_width: file.drive_capacity_width,
            drive_free_width: file.drive_free_width,
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
    // `settings.toml` → `settings.bak.toml` → defaults. A parse failure never writes:
    // overwriting a hand-edited file with the template loses the user's settings for good.
    match parse_file(&path) {
        Parse::Ok(settings) => return settings,
        Parse::Invalid => {
            CONFIG_REJECTED.store(true, Ordering::Relaxed);
            let _ = std::fs::copy(&path, dir.join("settings.rejected.toml"));
        }
        Parse::Missing => {}
    }
    if let Parse::Ok(settings) = parse_file(&dir.join("settings.bak.toml")) {
        return settings;
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
    // Materialize the commented template only when there is nothing to protect.
    let defaults = Settings::default();
    if !path.exists() {
        let _ = save(app, &defaults);
    }
    defaults
}

enum Parse {
    Ok(Settings),
    Invalid,
    Missing,
}

fn parse_file(path: &std::path::Path) -> Parse {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml_edit::de::from_str::<FileSettings>(&text) {
            Ok(file) => Parse::Ok(Settings::from(file).normalize()),
            Err(_) => Parse::Invalid,
        },
        Err(_) => Parse::Missing,
    }
}

pub fn save<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<(), ApiError> {
    let dir = store::config_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new("CONFIG_DIR", e.to_string()))?;
    let path = dir.join(FILE);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| TEMPLATE.to_string());
    let mut doc = match text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => {
            // An explicit save may legitimately repair a broken file, but the broken
            // content is still kept aside instead of being silently replaced.
            let _ = std::fs::copy(&path, dir.join("settings.rejected.toml"));
            TEMPLATE.parse().expect("template parses")
        }
    };

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
    set_value(&mut doc, "tab_font_family", Value::from(settings.tab_font_family.as_str()));
    set_value(&mut doc, "tab_font_size", Value::from(settings.tab_font_size as i64));
    set_value(&mut doc, "list_font_family", Value::from(settings.list_font_family.as_str()));
    set_value(&mut doc, "list_font_size", Value::from(settings.list_font_size as i64));
    set_value(&mut doc, "nav_font_family", Value::from(settings.nav_font_family.as_str()));
    set_value(&mut doc, "nav_font_size", Value::from(settings.nav_font_size as i64));
    set_value(
        &mut doc,
        "list_size_width",
        Value::from(settings.list_size_width as i64),
    );
    set_value(
        &mut doc,
        "list_time_width",
        Value::from(settings.list_time_width as i64),
    );
    set_value(
        &mut doc,
        "drive_capacity_width",
        Value::from(settings.drive_capacity_width as i64),
    );
    set_value(
        &mut doc,
        "drive_free_width",
        Value::from(settings.drive_free_width as i64),
    );

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

    /// Hand-edited values outside the UI's ranges must not leak into the layout/updater.
    #[test]
    fn invalid_general_values_normalize() {
        let parsed = Settings {
            language: "fr".into(),
            theme: "purple".into(),
            sidebar_width: 0,
            preview_width: 99_999,
            update_interval_hours: 48,
            ..Settings::default()
        }
        .normalize();
        assert_eq!(parsed.language, "zh-CN");
        assert_eq!(parsed.theme, "system");
        assert_eq!(parsed.sidebar_width, 140);
        assert_eq!(parsed.preview_width, 2000);
        assert_eq!(parsed.update_interval_hours, 24);

        let kept = Settings {
            language: "en".into(),
            theme: "dark".into(),
            sidebar_width: 300,
            preview_width: 480,
            update_interval_hours: 0,
            ..Settings::default()
        }
        .normalize();
        assert_eq!(kept.language, "en");
        assert_eq!(kept.theme, "dark");
        assert_eq!(kept.sidebar_width, 300);
        assert_eq!(kept.preview_width, 480);
        assert_eq!(kept.update_interval_hours, 0);
    }

    #[test]
    fn region_fonts_normalize() {        let parsed = Settings {
            tab_font_family: "  Consolas  ".to_string(),
            tab_font_size: 11,
            list_font_family: "".to_string(),
            list_font_size: 18,
            nav_font_family: "Bad; color: red".to_string(),
            nav_font_size: 9,
            ..Settings::default()
        }
        .normalize();
        assert_eq!(parsed.tab_font_family, "Consolas");
        assert_eq!(parsed.tab_font_size, 11);
        assert_eq!(parsed.list_font_family, "");
        assert_eq!(parsed.list_font_size, 18);
        // Invalid families and out-of-range sizes fall back to inherit/defaults.
        assert_eq!(parsed.nav_font_family, "");
        assert_eq!(parsed.nav_font_size, 12);

        for bad in [0, 9, 21, u32::MAX] {
            let parsed = Settings {
                tab_font_size: bad,
                ..Settings::default()
            }
            .normalize();
            assert_eq!(parsed.tab_font_size, 12, "{bad}");
        }
    }

    #[test]
    fn list_column_widths_fall_back() {
        let parsed = Settings {
            list_size_width: 56,
            list_time_width: 480,
            drive_capacity_width: 120,
            drive_free_width: 480,
            ..Settings::default()
        }
        .normalize();
        assert_eq!(parsed.list_size_width, 56);
        assert_eq!(parsed.list_time_width, 480);
        assert_eq!(parsed.drive_capacity_width, 120);
        assert_eq!(parsed.drive_free_width, 480);

        for bad in [0, 119, 481, u32::MAX] {
            let parsed = Settings {
                drive_capacity_width: bad,
                ..Settings::default()
            }
            .normalize();
            assert_eq!(parsed.drive_capacity_width, 150, "{bad}");
        }

        for bad in [0, 55, 481, u32::MAX] {
            let parsed = Settings {
                drive_free_width: bad,
                ..Settings::default()
            }
            .normalize();
            assert_eq!(parsed.drive_free_width, 84, "{bad}");
        }

        for bad in [0, 55, 481, u32::MAX] {
            let parsed = Settings {
                list_size_width: bad,
                ..Settings::default()
            }
            .normalize();
            assert_eq!(parsed.list_size_width, 96, "{bad}");
        }

        for bad in [0, 83, 481, u32::MAX] {
            let parsed = Settings {
                list_time_width: bad,
                ..Settings::default()
            }
            .normalize();
            assert_eq!(parsed.list_time_width, 142, "{bad}");
        }
    }

    #[test]
    fn template_and_inline_rules_parse() {
        let seed: FileSettings = toml_edit::de::from_str(TEMPLATE).unwrap();
        assert!(seed.external_preview.is_empty());
        assert_eq!(seed.list_size_width, 96);
        assert_eq!(seed.list_time_width, 142);
        assert_eq!(seed.drive_capacity_width, 150);
        assert_eq!(seed.drive_free_width, 84);
        assert_eq!(seed.tab_font_family, "");
        assert_eq!(seed.tab_font_size, 12);
        assert_eq!(seed.list_font_size, 13);
        assert_eq!(seed.nav_font_size, 12);
        let legacy: FileSettings = toml_edit::de::from_str("language = \"en\"").unwrap();
        assert_eq!(legacy.list_size_width, 96);
        assert_eq!(legacy.list_time_width, 142);
        assert_eq!(legacy.tab_font_size, 12);
        assert_eq!(legacy.list_font_size, 13);
        assert_eq!(legacy.nav_font_size, 12);

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
