//! Read-only extraction for Word (`.docx`) and Excel (`.xlsx`) previews.
//!
//! The files are OOXML zip containers, so previewing them needs no Office
//! installation and no new dependency: `zip` reads the parts and `quick-xml`
//! streams the XML. Only the first slice of a document is kept — the preview
//! panel is not a document viewer, and an unbounded parse would let a crafted
//! zip or a huge sheet stall the IPC thread.
//!
//! Word: `word/document.xml` gives paragraphs, runs, headings, lists and
//! tables. Styles, headers/footers, images and footnotes are intentionally
//! dropped; the preview only needs reading order.
//!
//! Excel: `xl/workbook.xml` + its rels locate the sheets; shared strings and
//! cell number formats are resolved so dates and percentages render as the
//! user sees them in Excel instead of raw serial numbers.

use std::collections::HashMap;
use std::io::{Read, Seek};

use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::Reader;
use serde::Serialize;

use crate::error::ApiError;

/// Largest decompressed XML part accepted from a container. OOXML files are
/// usually well below this; the cap is a zip-bomb guard, not a normal limit.
const MAX_XML_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DOC_BLOCKS: usize = 800;
const MAX_DOC_CHARS: usize = 400_000;
const MAX_TABLE_ROWS: usize = 80;
const MAX_TABLE_COLS: usize = 20;
const MAX_SHEETS: usize = 12;
// Parsed once and paged by the UI, so the cap bounds IPC payload and parse time
// rather than the rendered DOM. 40k cells covers most exports while keeping the
// JSON reply in the low-megabyte range.
const MAX_ROWS_PER_SHEET: usize = 2_000;
const MAX_COLS_PER_SHEET: usize = 80;
const MAX_CELLS_PER_SHEET: usize = 40_000;
/// Shared strings are referenced by index from cells, so one 8 MiB string
/// would be cloned into every referencing cell. Cap the text itself instead.
const MAX_SHARED_STRING_BYTES: usize = 4_096;
const MAX_CELL_BYTES: usize = 4_096;
/// Total rendered text budget per sheet; `MAX_CELLS_PER_SHEET` alone still
/// allows 40k long strings to be cloned into the IPC payload.
const MAX_SHEET_BYTES: usize = 1_000_000;

/// A formatted span inside a Word paragraph.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct DocRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

/// One block in reading order. Word tables carry `rows`; every other kind
/// carries `text` plus the styled `runs` needed to render bold/italic spans.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DocBlock {
    /// `heading`, `paragraph`, `list` or `table`.
    pub kind: String,
    /// Heading level (1–6) or list indent level (1-based).
    pub level: Option<u8>,
    pub text: String,
    pub runs: Vec<DocRun>,
    pub rows: Vec<Vec<String>>,
    /// True when Word marked the first table row as a header.
    pub header: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct OfficeDoc {
    pub blocks: Vec<DocBlock>,
    /// The document continued past the block/character cap.
    pub truncated: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SheetPreview {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    /// The sheet, or a later sheet of the workbook, was cut off by a cap.
    pub truncated: bool,
}

/// Reads a Word document into preview blocks. Malformed XML yields the blocks
/// parsed so far with `truncated` set rather than failing the whole preview.
pub fn docx(path: &str) -> Result<OfficeDoc, ApiError> {
    let file = std::fs::File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ApiError::new("PREVIEW", format!("docx: {e}")))?;
    let (xml, cut) = read_entry_capped(&mut archive, "word/document.xml", MAX_XML_BYTES)
        .ok_or_else(|| ApiError::new("PREVIEW", "docx: missing word/document.xml"))?;
    let style_levels = read_entry(&mut archive, "word/styles.xml", MAX_XML_BYTES)
        .map(|xml| parse_heading_styles(&xml))
        .unwrap_or_default();
    Ok(parse_docx(&xml, style_levels, cut))
}

/// Reads every sheet (up to the sheet cap) into preview tables.
pub fn xlsx(path: &str) -> Result<Vec<SheetPreview>, ApiError> {
    let file = std::fs::File::open(path).map_err(|e| ApiError::from_io(path, &e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ApiError::new("PREVIEW", format!("xlsx: {e}")))?;
    parse_xlsx(&mut archive)
}

fn read_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    max: u64,
) -> Option<Vec<u8>> {
    read_entry_capped(archive, name, max).map(|(bytes, _)| bytes)
}

/// Reads up to `max` decompressed bytes and reports whether the part was
/// longer. The truncation flag matters: a cut at a tag boundary parses cleanly
/// but the document is still incomplete.
fn read_entry_capped<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    max: u64,
) -> Option<(Vec<u8>, bool)> {
    let entry = archive.by_name(name).ok()?;
    let truncated = entry.size() > max;
    let mut bytes = Vec::new();
    entry.take(max).read_to_end(&mut bytes).ok()?;
    Some((bytes, truncated))
}

/// Reads an attribute by local name, ignoring the namespace prefix. Prefixes
/// are chosen by the producer, so `w:val` in one file may be `wx:val` in
/// another while meaning the same thing.
fn attr(element: &BytesStart<'_>, name: &str) -> Option<String> {
    let local = name.rsplit(':').next().unwrap_or(name);
    element.attributes().flatten().find_map(|value| {
        if value.key.local_name().as_ref() != local {
            return None;
        }
        value
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .ok()
            .map(|value| value.into_owned())
    })
}

/// Resolves a `&...;` reference encountered inside text content.
fn reference_text(reference: &BytesRef<'_>) -> Option<String> {
    match reference.resolve_char_ref() {
        Ok(Some(ch)) => Some(ch.to_string()),
        Ok(None) => quick_xml::escape::resolve_predefined_entity(reference.as_ref())
            .map(str::to_string),
        Err(_) => None,
    }
}

/// Appends at most `max` bytes of `text`; returns true when the input was
/// clipped. Capping by bytes bounds memory without breaking UTF-8.
fn push_limited(target: &mut String, text: &str, max: usize) -> bool {
    let room = max.saturating_sub(target.len());
    if text.len() <= room {
        target.push_str(text);
        false
    } else {
        let mut cut = room;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        target.push_str(&text[..cut]);
        true
    }
}

// ---------------------------------------------------------------------------
// Word
// ---------------------------------------------------------------------------

#[derive(Default)]
struct DocBuilder {
    doc: OfficeDoc,
    table_depth: usize,
    table_rows: Vec<Vec<String>>,
    table_header: bool,
    row: Vec<String>,
    row_header: bool,
    cell: String,
    para: String,
    runs: Vec<DocRun>,
    run: Option<DocRun>,
    in_rpr: bool,
    in_text: bool,
    in_numpr: bool,
    style: String,
    outline_level: Option<u8>,
    list_level: Option<u8>,
    chars: usize,
    /// Style ids come from the producer's locale; `styles.xml` maps them to the
    /// built-in English style names.
    style_levels: HashMap<String, u8>,
    /// DrawingML (`<w:drawing>`) carries `<a:p>`/`<a:t>` that are not document
    /// paragraphs; letting them through duplicates or corrupts body text.
    drawing_depth: usize,
}

impl DocBuilder {
    fn push_text(&mut self, text: &str) {
        if text.is_empty() || self.chars >= MAX_DOC_CHARS {
            if !text.is_empty() {
                self.doc.truncated = true;
            }
            return;
        }
        let room = MAX_DOC_CHARS - self.chars;
        let clipped = if text.chars().count() > room {
            self.doc.truncated = true;
            text.chars().take(room).collect::<String>()
        } else {
            text.to_string()
        };
        self.chars += clipped.chars().count();
        if self.table_depth > 0 {
            self.cell.push_str(&clipped);
        } else {
            self.para.push_str(&clipped);
            if let Some(run) = self.run.as_mut() {
                run.text.push_str(&clipped);
            }
        }
    }

    fn finish_run(&mut self) {
        if let Some(run) = self.run.take() {
            if !run.text.is_empty() {
                self.runs.push(run);
            }
        }
    }

    fn finish_paragraph(&mut self) {
        self.finish_run();
        if self.table_depth > 0 {
            // A paragraph break inside a table cell is a visible line break.
            if !self.cell.is_empty() && !self.cell.ends_with('\n') {
                self.cell.push('\n');
            }
            return;
        }
        let text = self.para.trim().to_string();
        let heading = heading_level(&self.style, self.outline_level, &self.style_levels);
        if text.is_empty() {
            self.para.clear();
            self.runs.clear();
            return;
        }
        if self.doc.blocks.len() >= MAX_DOC_BLOCKS {
            self.doc.truncated = true;
            self.para.clear();
            self.runs.clear();
            return;
        }
        let (kind, level) = if let Some(level) = heading {
            ("heading", Some(level))
        } else if self.in_numpr || self.list_level.is_some() {
            ("list", self.list_level.map(|level| level.saturating_add(1)))
        } else {
            ("paragraph", None)
        };
        let mut runs = std::mem::take(&mut self.runs);
        if runs.is_empty() {
            runs.push(DocRun {
                text: text.clone(),
                bold: false,
                italic: false,
            });
        }
        self.doc.blocks.push(DocBlock {
            kind: kind.to_string(),
            level,
            text,
            runs,
            rows: Vec::new(),
            header: false,
        });
        self.para.clear();
    }

    fn finish_table(&mut self) {
        if self.table_depth > 0 {
            return;
        }
        let rows = std::mem::take(&mut self.table_rows);
        if rows.iter().all(|row| row.iter().all(String::is_empty)) {
            self.table_header = false;
            return;
        }
        if self.doc.blocks.len() >= MAX_DOC_BLOCKS {
            self.doc.truncated = true;
            return;
        }
        self.doc.blocks.push(DocBlock {
            kind: "table".to_string(),
            level: None,
            text: String::new(),
            runs: Vec::new(),
            rows,
            header: self.table_header,
        });
        self.table_header = false;
    }
}

fn heading_level(
    style: &str,
    outline: Option<u8>,
    style_levels: &HashMap<String, u8>,
) -> Option<u8> {
    if let Some(level) = outline {
        if level < 9 {
            return Some(level.saturating_add(1).min(6));
        }
    }
    if let Some(level) = style_levels.get(style) {
        return Some(*level);
    }
    let lower = style.to_ascii_lowercase();
    if lower == "title" {
        return Some(1);
    }
    if lower == "subtitle" {
        return Some(2);
    }
    let digits: String = lower
        .strip_prefix("heading")
        .unwrap_or("")
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits
        .parse::<u8>()
        .ok()
        .map(|level| level.clamp(1, 6))
}

/// Maps paragraph style ids to heading levels from `word/styles.xml`.
///
/// A localized Word writes ids such as `1` or `Titre1` while keeping the
/// built-in English style name (`heading 1`), so the name is the stable key.
fn parse_heading_styles(xml: &[u8]) -> HashMap<String, u8> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut levels = HashMap::new();
    let mut current: Option<String> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                "style" => current = attr(&element, "styleId"),
                "name" => record_heading(&mut levels, current.as_deref(), &element),
                _ => {}
            },
            Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == "name" {
                    record_heading(&mut levels, current.as_deref(), &element);
                }
            }
            Ok(Event::End(element)) => {
                if element.local_name().as_ref() == "style" {
                    current = None;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    levels
}

fn record_heading(levels: &mut HashMap<String, u8>, id: Option<&str>, element: &BytesStart<'_>) {
    let (Some(id), Some(name)) = (id, attr(element, "val")) else {
        return;
    };
    if let Some(level) = named_heading(&name) {
        levels.insert(id.to_string(), level);
    }
}

fn named_heading(name: &str) -> Option<u8> {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == "title" {
        return Some(1);
    }
    if lower == "subtitle" {
        return Some(2);
    }
    let digits = lower
        .strip_prefix("heading")
        .or_else(|| trimmed.strip_prefix("标题"))?;
    let level: u8 = digits.trim().parse().ok()?;
    Some(level.clamp(1, 6))
}

fn parse_docx(xml: &[u8], style_levels: HashMap<String, u8>, truncated: bool) -> OfficeDoc {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut builder = DocBuilder {
        style_levels,
        doc: OfficeDoc {
            truncated,
            ..OfficeDoc::default()
        },
        ..DocBuilder::default()
    };
    let mut buf = Vec::new();

    loop {
        let event = reader.read_event_into(&mut buf);
        let event = match event {
            Ok(Event::Eof) => break,
            Ok(event) => event,
            Err(_) => {
                builder.doc.truncated = true;
                break;
            }
        };
        match event {
            Event::Start(element) => {
                let name = element.local_name().as_ref().to_string();
                doc_start(&mut builder, &name, &element);
            }
            Event::Empty(element) => {
                let name = element.local_name().as_ref().to_string();
                doc_empty(&mut builder, &name, &element);
            }
            Event::Text(text) => {
                if builder.in_text {
                    let raw = text.xml10_content();
                    let value = quick_xml::escape::unescape(&raw)
                        .map(|value| value.into_owned())
                        .unwrap_or_else(|_| raw.into_owned());
                    builder.push_text(&value);
                }
            }
            Event::CData(text) => {
                if builder.in_text {
                    builder.push_text(&text.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                if builder.in_text {
                    if let Some(text) = reference_text(&reference) {
                        builder.push_text(&text);
                    }
                }
            }
            Event::End(element) => {
                let name = element.local_name().as_ref().to_string();
                if name == "drawing" {
                    builder.drawing_depth = builder.drawing_depth.saturating_sub(1);
                } else if builder.drawing_depth == 0 {
                    match name.as_str() {
                        "p" => builder.finish_paragraph(),
                        "r" => builder.finish_run(),
                        "rPr" => builder.in_rpr = false,
                        "numPr" => builder.in_numpr = false,
                        "t" => builder.in_text = false,
                        "tc" if builder.table_depth == 1 => {
                            let mut cell = builder.cell.trim_end_matches('\n').trim().to_string();
                            cell.shrink_to_fit();
                            if builder.row.len() < MAX_TABLE_COLS {
                                builder.row.push(cell);
                            } else {
                                builder.doc.truncated = true;
                            }
                            builder.cell.clear();
                        }
                        "tr" if builder.table_depth == 1 => {
                            if builder.row_header {
                                builder.table_header = true;
                            }
                            if builder.table_rows.len() < MAX_TABLE_ROWS {
                                builder.table_rows.push(std::mem::take(&mut builder.row));
                            } else {
                                builder.doc.truncated = true;
                            }
                            builder.row_header = false;
                        }
                        "tbl" => {
                            builder.table_depth = builder.table_depth.saturating_sub(1);
                            builder.finish_table();
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    builder.doc
}

fn doc_start(builder: &mut DocBuilder, name: &str, element: &BytesStart<'_>) {
    if name == "drawing" {
        builder.drawing_depth += 1;
        return;
    }
    if builder.drawing_depth > 0 {
        return;
    }
    match name {
        "p" => {
            builder.para.clear();
            builder.runs.clear();
            builder.style.clear();
            builder.outline_level = None;
            builder.list_level = None;
            builder.in_numpr = false;
        }
        "r" => {
            builder.finish_run();
            builder.run = Some(DocRun::default());
            builder.in_rpr = false;
        }
        "rPr" => builder.in_rpr = true,
        "numPr" => builder.in_numpr = true,
        "pStyle" => {
            if let Some(value) = attr(element, "w:val") {
                builder.style = value;
            }
        }
        "outlineLvl" => {
            if let Some(level) = attr(element, "w:val").and_then(|value| value.parse::<u8>().ok()) {
                builder.outline_level = Some(level);
            }
        }
        "ilvl" if builder.in_numpr => {
            if let Some(level) = attr(element, "w:val").and_then(|value| value.parse::<u8>().ok()) {
                builder.list_level = Some(level);
            }
        }
        "t" => builder.in_text = true,
        "tbl" => {
            builder.table_depth += 1;
            if builder.table_depth == 1 {
                builder.table_rows.clear();
                builder.row.clear();
                builder.cell.clear();
            }
        }
        "tr" if builder.table_depth == 1 => {
            builder.row.clear();
            builder.row_header = false;
        }
        "tc" if builder.table_depth == 1 => builder.cell.clear(),
        "tblHeader" if builder.table_depth == 1 => builder.row_header = true,
        _ => {}
    }
}

fn doc_empty(builder: &mut DocBuilder, name: &str, element: &BytesStart<'_>) {
    if builder.drawing_depth > 0 {
        return;
    }
    match name {
        "pStyle" => {
            if let Some(value) = attr(element, "w:val") {
                builder.style = value;
            }
        }
        "outlineLvl" => {
            if let Some(level) = attr(element, "w:val").and_then(|value| value.parse::<u8>().ok()) {
                builder.outline_level = Some(level);
            }
        }
        "ilvl" if builder.in_numpr => {
            if let Some(level) = attr(element, "w:val").and_then(|value| value.parse::<u8>().ok()) {
                builder.list_level = Some(level);
            }
        }
        "tblHeader" if builder.table_depth == 1 => builder.row_header = true,
        "b" if builder.in_rpr && builder.run.is_some() => {
            let on = attr(element, "w:val")
                .map(|value| !matches!(value.as_str(), "0" | "false" | "off"))
                .unwrap_or(true);
            if let Some(run) = builder.run.as_mut() {
                run.bold = on;
            }
        }
        "i" if builder.in_rpr && builder.run.is_some() => {
            let on = attr(element, "w:val")
                .map(|value| !matches!(value.as_str(), "0" | "false" | "off"))
                .unwrap_or(true);
            if let Some(run) = builder.run.as_mut() {
                run.italic = on;
            }
        }
        "tab" => builder.push_text("\t"),
        "br" | "cr" => builder.push_text("\n"),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Excel
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum CellFmt {
    General,
    Date { time: bool },
    Percent,
}

fn parse_xlsx<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Result<Vec<SheetPreview>, ApiError> {
    let workbook_xml = read_entry(archive, "xl/workbook.xml", MAX_XML_BYTES)
        .ok_or_else(|| ApiError::new("PREVIEW", "xlsx: missing xl/workbook.xml"))?;
    let workbook = parse_workbook(&workbook_xml);
    if workbook.sheets.is_empty() {
        return Ok(Vec::new());
    }
    let more_sheets = workbook.sheets.len() > MAX_SHEETS;
    let rels = read_entry(archive, "xl/_rels/workbook.xml.rels", MAX_XML_BYTES)
        .map(|xml| parse_rels(&xml))
        .unwrap_or_default();
    let (shared, shared_cut) = read_entry_capped(archive, "xl/sharedStrings.xml", MAX_XML_BYTES)
        .map(|(xml, cut)| {
            let (strings, clipped) = parse_shared_strings(&xml);
            (strings, cut || clipped)
        })
        .unwrap_or_default();
    let formats = read_entry(archive, "xl/styles.xml", MAX_XML_BYTES)
        .map(|xml| parse_styles(&xml))
        .unwrap_or_default();

    let mut out = Vec::new();
    for (index, (name, rid)) in workbook.sheets.into_iter().take(MAX_SHEETS).enumerate() {
        let path = rid
            .as_deref()
            .and_then(|rid| rels.get(rid))
            .cloned()
            .unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", index + 1));
        let Some((xml, cut)) = read_entry_capped(archive, &path, MAX_XML_BYTES) else {
            continue;
        };
        let (rows, truncated) =
            parse_sheet(&xml, &shared, &formats, workbook.date1904, cut || shared_cut);
        out.push(SheetPreview {
            name,
            rows,
            truncated,
        });
    }
    // The tabs stop at MAX_SHEETS; flag the last one so the UI can say the
    // workbook continues instead of silently pretending this is everything.
    if more_sheets {
        if let Some(last) = out.last_mut() {
            last.truncated = true;
        }
    }
    Ok(out)
}

#[derive(Default)]
struct Workbook {
    sheets: Vec<(String, Option<String>)>,
    date1904: bool,
}

fn parse_workbook(xml: &[u8]) -> Workbook {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut workbook = Workbook::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                match element.local_name().as_ref() {
                    "sheet" => {
                        let name = attr(&element, "name").unwrap_or_default();
                        let rid = attr(&element, "r:id").or_else(|| attr(&element, "id"));
                        if !name.is_empty() {
                            workbook.sheets.push((name, rid));
                        }
                    }
                    "workbookPr" => {
                        if let Some(value) = attr(&element, "date1904") {
                            let value = value.trim();
                            workbook.date1904 =
                                value == "1" || value.eq_ignore_ascii_case("true");
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    workbook
}

fn parse_rels(xml: &[u8]) -> HashMap<String, String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut rels = HashMap::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == "Relationship" {
                    if let (Some(id), Some(target)) =
                        (attr(&element, "Id"), attr(&element, "Target"))
                    {
                        rels.insert(id, normalize_target(&target));
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    rels
}

fn normalize_target(target: &str) -> String {
    let target = target.replace('\\', "/");
    let absolute = target.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if absolute || joined.starts_with("xl/") {
        joined
    } else {
        format!("xl/{joined}")
    }
}

/// Returns the strings plus whether any was clipped at [`MAX_SHARED_STRING_BYTES`], so the
/// preview can say the sheet is incomplete instead of silently showing a prefix.
fn parse_shared_strings(xml: &[u8]) -> (Vec<String>, bool) {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut strings = Vec::new();
    let mut clipped = false;
    let mut current: Option<String> = None;
    let mut in_text = false;
    let mut phonetic = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) => {
                let name = element.local_name().as_ref().to_string();
                match name.as_str() {
                    "si" => current = Some(String::new()),
                    "rPh" => phonetic = true,
                    "t" => in_text = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == "si" {
                    strings.push(String::new());
                }
            }
            Ok(Event::Text(text)) => {
                if in_text && !phonetic {
                    if let Some(current) = current.as_mut() {
                        let raw = text.xml10_content();
                        let value = quick_xml::escape::unescape(&raw)
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|_| raw.into_owned());
                        clipped |= push_limited(current, &value, MAX_SHARED_STRING_BYTES);
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if in_text && !phonetic {
                    if let (Some(current), Some(text)) = (current.as_mut(), reference_text(&reference))
                    {
                        clipped |= push_limited(current, &text, MAX_SHARED_STRING_BYTES);
                    }
                }
            }
            Ok(Event::End(element)) => match element.local_name().as_ref() {
                "si" => {
                    if let Some(value) = current.take() {
                        strings.push(value);
                    }
                    phonetic = false;
                }
                "rPh" => phonetic = false,
                "t" => in_text = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => {
                // A cut XML part is a truncated table, not a clean end.
                clipped = true;
                break;
            }
            _ => {}
        }
        buf.clear();
    }
    (strings, clipped)
}

#[derive(Default)]
struct Formats {
    custom: HashMap<u32, String>,
    xf: Vec<u32>,
}

impl Formats {
    fn resolve(&self, style: usize) -> CellFmt {
        let id = self.xf.get(style).copied().unwrap_or(0);
        classify(id, self.custom.get(&id))
    }
}

fn parse_styles(xml: &[u8]) -> Formats {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut formats = Formats::default();
    let mut in_cell_xfs = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) => {
                let name = element.local_name().as_ref().to_string();
                match name.as_str() {
                    "cellXfs" => in_cell_xfs = true,
                    "xf" if in_cell_xfs => formats.xf.push(num_fmt_id(&element)),
                    "numFmt" => add_custom_format(&mut formats, &element),
                    _ => {}
                }
            }
            Ok(Event::Empty(element)) => {
                let name = element.local_name().as_ref().to_string();
                match name.as_str() {
                    "xf" if in_cell_xfs => formats.xf.push(num_fmt_id(&element)),
                    "numFmt" => add_custom_format(&mut formats, &element),
                    _ => {}
                }
            }
            Ok(Event::End(element)) => {
                if element.local_name().as_ref() == "cellXfs" {
                    in_cell_xfs = false;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    formats
}

fn num_fmt_id(element: &BytesStart<'_>) -> u32 {
    attr(element, "numFmtId")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn add_custom_format(formats: &mut Formats, element: &BytesStart<'_>) {
    if let (Some(id), Some(code)) = (
        attr(element, "numFmtId").and_then(|value| value.parse::<u32>().ok()),
        attr(element, "formatCode"),
    ) {
        formats.custom.insert(id, code);
    }
}

/// Maps an Excel number format to the only presentation the preview needs.
/// Built-in ids are enough for the common date/percent cases; custom formats
/// are reduced to their letter skeleton so `yyyy-mm-dd"T"hh:mm` still counts
/// as a datetime and `0.00%` as a percentage.
fn classify(id: u32, code: Option<&String>) -> CellFmt {
    match id {
        9 | 10 => return CellFmt::Percent,
        14..=17 => return CellFmt::Date { time: false },
        18..=22 | 45..=47 => return CellFmt::Date { time: true },
        _ => {}
    }
    let Some(code) = code else {
        return CellFmt::General;
    };
    let mut clean = String::new();
    let mut quoted = false;
    let mut chars = code.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quoted = !quoted,
            '\\' | '_' | '*' => {
                chars.next();
            }
            '[' if !quoted => {
                for inner in chars.by_ref() {
                    if inner == ']' {
                        break;
                    }
                }
            }
            _ if quoted => {}
            _ => clean.push(ch),
        }
    }
    let lower = clean.to_ascii_lowercase();
    if lower.contains('%') {
        CellFmt::Percent
    } else if lower.contains('y')
        || lower.contains('d')
        || lower.contains('h')
        || lower.contains('s')
    {
        CellFmt::Date {
            time: lower.contains('h') || lower.contains('s'),
        }
    } else {
        CellFmt::General
    }
}

fn parse_sheet(
    xml: &[u8],
    shared: &[String],
    formats: &Formats,
    date1904: bool,
    cut: bool,
) -> (Vec<Vec<String>>, bool) {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut row_index = 0usize;
    let mut next_col = 0usize;
    let mut truncated = cut;
    let mut rendered = 0usize;
    let mut cells = 0usize;
    let mut in_value = false;
    let mut in_inline = false;
    let mut in_text = false;
    let mut value = String::new();
    let mut cell_kind = String::new();
    let mut cell_style = 0usize;
    let mut cell_col = 0usize;
    let mut buf = Vec::new();

    'read: loop {
        let event = match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(event) => event,
            Err(_) => {
                truncated = true;
                break;
            }
        };
        match event {
            Event::Start(element) => {
                let name = element.local_name().as_ref().to_string();
                match name.as_str() {
                    "row" => {
                        row_index = attr(&element, "r")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(row_index + 1);
                        if row_index > MAX_ROWS_PER_SHEET {
                            truncated = true;
                            break 'read;
                        }
                        // Leading empty rows matter for layout only up to the cap.
                        while rows.len() + 1 < row_index && rows.len() < MAX_ROWS_PER_SHEET {
                            rows.push(Vec::new());
                        }
                        row.clear();
                        next_col = 0;
                    }
                    "c" => {
                        cell_kind = attr(&element, "t").unwrap_or_default();
                        cell_style = attr(&element, "s")
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(0);
                        let reference = attr(&element, "r");
                        cell_col = reference
                            .as_deref()
                            .map(cell_column)
                            .unwrap_or(next_col);
                        next_col = cell_col.saturating_add(1);
                        value.clear();
                    }
                    "v" => in_value = true,
                    "is" => in_inline = true,
                    "t" if in_inline => in_text = true,
                    _ => {}
                }
            }
            Event::Empty(element) => {
                if element.local_name().as_ref() == "row" {
                    let index = attr(&element, "r")
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(row_index + 1);
                    if index > MAX_ROWS_PER_SHEET || rows.len() >= MAX_ROWS_PER_SHEET {
                        truncated = true;
                    } else {
                        while rows.len() + 1 < index {
                            rows.push(Vec::new());
                        }
                        rows.push(Vec::new());
                        row_index = rows.len();
                    }
                }
            }
            Event::Text(text) => {
                if in_value || in_text {
                    let raw = text.xml10_content();
                    let decoded = quick_xml::escape::unescape(&raw)
                        .map(|value| value.into_owned())
                        .unwrap_or_else(|_| raw.into_owned());
                    if push_limited(&mut value, &decoded, MAX_CELL_BYTES) {
                        truncated = true;
                    }
                }
            }
            Event::GeneralRef(reference) => {
                if in_value || in_text {
                    if let Some(decoded) = reference_text(&reference) {
                        if push_limited(&mut value, &decoded, MAX_CELL_BYTES) {
                            truncated = true;
                        }
                    }
                }
            }
            Event::End(element) => {
                let name = element.local_name().as_ref().to_string();
                match name.as_str() {
                    "v" => in_value = false,
                    "t" => in_text = false,
                    "is" => in_inline = false,
                    "c" => {
                        cells += 1;
                        if cells > MAX_CELLS_PER_SHEET {
                            truncated = true;
                            break 'read;
                        }
                        if cell_col >= MAX_COLS_PER_SHEET {
                            truncated = true;
                            value.clear();
                        } else {
                            let text =
                                render_cell(&cell_kind, &value, shared, formats, cell_style, date1904);
                            if rendered.saturating_add(text.len()) > MAX_SHEET_BYTES {
                                truncated = true;
                                break 'read;
                            }
                            rendered += text.len();
                            place_cell(&mut row, cell_col, text);
                        }
                        value.clear();
                    }
                    "row" => {
                        if rows.len() >= MAX_ROWS_PER_SHEET {
                            truncated = true;
                            break 'read;
                        }
                        trim_row(&mut row);
                        rows.push(std::mem::take(&mut row));
                        row_index = rows.len();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        buf.clear();
    }

    while rows.last().is_some_and(|row| row.is_empty()) {
        rows.pop();
    }
    // Reaching exactly the cap is not truncation; only data past it is. The loop
    // above marks `truncated` when it stops early, so trust that flag.
    (rows, truncated)
}

fn cell_column(reference: &str) -> usize {
    let mut column = 0usize;
    for ch in reference.chars() {
        if ch.is_ascii_alphabetic() {
            column = column
                .saturating_mul(26)
                .saturating_add((ch.to_ascii_uppercase() as u8 - b'A' + 1) as usize);
        } else {
            break;
        }
    }
    column.saturating_sub(1)
}

fn place_cell(row: &mut Vec<String>, column: usize, value: String) {
    if column > MAX_COLS_PER_SHEET {
        return;
    }
    while row.len() < column {
        row.push(String::new());
    }
    if row.len() == column {
        row.push(value);
    } else {
        row[column] = value;
    }
}

fn trim_row(row: &mut Vec<String>) {
    while row.last().is_some_and(String::is_empty) {
        row.pop();
    }
}

fn render_cell(
    kind: &str,
    raw: &str,
    shared: &[String],
    formats: &Formats,
    style: usize,
    date1904: bool,
) -> String {
    match kind {
        "s" => raw
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|index| shared.get(index))
            .cloned()
            .unwrap_or_default(),
        "inlineStr" | "str" => raw.to_string(),
        "b" => {
            if raw.trim() == "1" || raw.eq_ignore_ascii_case("true") {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        "e" => raw.to_string(),
        "d" => raw.replace('T', " ").trim_end_matches('Z').to_string(),
        _ => format_serial(raw, formats.resolve(style), date1904),
    }
}

fn format_serial(raw: &str, format: CellFmt, date1904: bool) -> String {
    let Ok(number) = raw.trim().parse::<f64>() else {
        return raw.to_string();
    };
    match format {
        CellFmt::General => raw.trim().to_string(),
        CellFmt::Percent => format!("{}%", trim_float(number * 100.0)),
        CellFmt::Date { time } => excel_date(number, time, date1904),
    }
}

fn trim_float(value: f64) -> String {
    if (value.fract()).abs() < 1e-9 {
        return format!("{}", value as i64);
    }
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Excel caps dates at 9999-12-31 (serial 2,958,465 in the 1900 system).
/// Beyond that the day count would overflow the calendar conversion.
const MAX_EXCEL_SERIAL: f64 = 2_958_465.0;

/// Excel stores dates as days since 1899-12-30 (with the fictitious 1900-02-29
/// at serial 60) or, when the workbook sets `date1904`, since 1904-01-01.
/// Converting through a Unix day count keeps the arithmetic in integers;
/// serials below 60 are shifted by one to match real calendar dates.
fn excel_date(serial: f64, with_time: bool, date1904: bool) -> String {
    if !serial.is_finite() || serial < 0.0 || serial > MAX_EXCEL_SERIAL {
        return format!("{serial}");
    }
    // Serial 1462 in the 1900 system is 1904-01-01.
    let adjusted = if date1904 {
        serial + 1462.0
    } else if serial < 60.0 {
        serial + 1.0
    } else {
        serial
    };
    let mut days = adjusted.floor() as i64 - 25569; // days since 1970-01-01
    let mut seconds = (serial.fract() * 86400.0).round() as i64;
    if seconds >= 86400 {
        // Rounding a fraction just below 1.0 lands on the next day.
        seconds -= 86400;
        days += 1;
    }
    let (hour, minute, second) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    let clock = format!("{hour:02}:{minute:02}:{second:02}");
    // A serial below 1 has no date part; Excel shows a clock for time formats.
    if with_time && serial < 1.0 {
        return clock;
    }
    let (year, month, day) = civil_from_days(days);
    if with_time {
        format!("{year:04}-{month:02}-{day:02} {clock}")
    } else {
        format!("{year:04}-{month:02}-{day:02}")
    }
}

/// Howard Hinnant's civil-from-days algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Builds an `.xlsx` from in-memory parts. Tests use a `Cursor`, production
/// opens a file, so the parser never needs to know which container it reads.
#[cfg(test)]
fn archive_from_parts(parts: &[(&str, &[u8])]) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
    use std::io::Cursor;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in parts {
        use std::io::Write;
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    let cursor = writer.finish().unwrap();
    zip::ZipArchive::new(cursor).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Overview</w:t></w:r></w:p>
    <w:p><w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>world</w:t></w:r><w:r><w:t>!</w:t></w:r></w:p>
    <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/></w:numPr></w:pPr><w:r><w:t>Item</w:t></w:r></w:p>
    <w:tbl>
      <w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:p><w:r><w:t>Name</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Score</w:t></w:r></w:p></w:tc></w:tr>
      <w:tr><w:tc><w:p><w:r><w:t>Alice</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>42</w:t></w:r></w:p></w:tc></w:tr>
    </w:tbl>
    <w:p><w:pPr><w:outlineLvl w:val="2"/></w:pPr><w:r><w:t>Details</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

    #[test]
    fn docx_keeps_headings_runs_lists_and_tables() {
        let doc = parse_docx(DOC.as_bytes(), HashMap::new(), false);
        assert!(!doc.truncated);
        assert_eq!(doc.blocks.len(), 5);

        assert_eq!(doc.blocks[0].kind, "heading");
        assert_eq!(doc.blocks[0].level, Some(1));
        assert_eq!(doc.blocks[0].text, "Overview");

        assert_eq!(doc.blocks[1].kind, "paragraph");
        assert_eq!(doc.blocks[1].text, "Hello world!");
        assert_eq!(doc.blocks[1].runs.len(), 3);
        assert!(doc.blocks[1].runs[1].bold);
        assert!(doc.blocks[1].runs[1].italic);
        assert!(!doc.blocks[1].runs[0].bold);

        assert_eq!(doc.blocks[2].kind, "list");
        assert_eq!(doc.blocks[2].level, Some(2));

        let table = &doc.blocks[3];
        assert_eq!(table.kind, "table");
        assert!(table.header);
        assert_eq!(table.rows, vec![vec!["Name", "Score"], vec!["Alice", "42"]]);

        assert_eq!(doc.blocks[4].kind, "heading");
        assert_eq!(doc.blocks[4].level, Some(3));
        assert_eq!(doc.blocks[4].text, "Details");
    }

    #[test]
    fn docx_outline_level_wins_over_style() {
        let xml = br#"<w:document xmlns:w="w"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/><w:outlineLvl w:val="2"/></w:pPr><w:r><w:t>X</w:t></w:r></w:p></w:body></w:document>"#;
        let doc = parse_docx(xml, HashMap::new(), false);
        assert_eq!(doc.blocks[0].level, Some(3));
    }

    const WORKBOOK: &str = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Second" sheetId="2" r:id="rId2"/></sheets>
</workbook>"#;

    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="worksheet" Target="/xl/worksheets/sheet2.xml"/>
</Relationships>"#;

    const SHARED: &str = r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <si><t>Name</t></si><si><t>Date</t></si><si><t>Alice</t></si>
  <si><r><t>Rich</t></r><r><t> Text</t></r><rPh><t>ignored</t></rPh></si>
</sst>"#;

    const STYLES: &str = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <numFmts><numFmt numFmtId="164" formatCode="yyyy/mm/dd"/></numFmts>
  <cellXfs count="4"><xf numFmtId="0"/><xf numFmtId="14"/><xf numFmtId="9"/><xf numFmtId="164"/></cellXfs>
</styleSheet>"#;

    const SHEET1: &str = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
    <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
    <row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>45678</v></c></row>
    <row r="4"><c r="B4" s="1"><v>45678</v></c><c r="C4" s="2"><v>0.25</v></c><c r="D4" t="inlineStr"><is><t>inline</t></is></c></row>
    <row r="5"><c r="A5" t="s"><v>3</v></c><c r="B5" s="3"><v>45678</v></c></row>
  </sheetData>
</worksheet>"#;

    const SHEET2: &str = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="b"><v>1</v></c></row></sheetData></worksheet>"#;

    #[test]
    fn xlsx_resolves_sheets_strings_dates_and_percentages() {
        let mut archive = archive_from_parts(&[
            ("xl/workbook.xml", WORKBOOK.as_bytes()),
            ("xl/_rels/workbook.xml.rels", RELS.as_bytes()),
            ("xl/sharedStrings.xml", SHARED.as_bytes()),
            ("xl/styles.xml", STYLES.as_bytes()),
            ("xl/worksheets/sheet1.xml", SHEET1.as_bytes()),
            ("xl/worksheets/sheet2.xml", SHEET2.as_bytes()),
        ]);
        let sheets = parse_xlsx(&mut archive).unwrap();
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0].name, "Data");
        assert_eq!(sheets[0].rows[0], vec!["Name", "Date"]);
        assert_eq!(sheets[0].rows[1], vec!["Alice", "45678"]);
        // Row 3 is absent in the source and stays empty so row 4 lines up.
        assert!(sheets[0].rows[2].is_empty());
        assert_eq!(sheets[0].rows[3][1], "2025-01-21");
        assert_eq!(sheets[0].rows[3][2], "25%");
        assert_eq!(sheets[0].rows[3][3], "inline");
        assert_eq!(sheets[0].rows[4][0], "Rich Text");
        assert_eq!(sheets[0].rows[4][1], "2025-01-21");
        assert_eq!(sheets[1].name, "Second");
        assert_eq!(sheets[1].rows[0], vec!["TRUE"]);
    }

    #[test]
    fn xlsx_handles_shared_string_rich_text_without_phonetics() {
        let (strings, clipped) = parse_shared_strings(SHARED.as_bytes());
        assert_eq!(strings[3], "Rich Text");
        assert_eq!(strings.len(), 4);
        assert!(!clipped);
    }

    #[test]
    fn a_clipped_shared_string_is_reported() {
        let long = "x".repeat(MAX_SHARED_STRING_BYTES + 10);
        let xml = format!("<sst><si><t>{long}</t></si></sst>");
        let (strings, clipped) = parse_shared_strings(xml.as_bytes());
        assert!(clipped);
        assert_eq!(strings[0].len(), MAX_SHARED_STRING_BYTES);
    }

    #[test]
    fn sheet_stops_at_the_row_cap() {
        let mut xml = String::from("<worksheet><sheetData>");
        for row in 1..=MAX_ROWS_PER_SHEET + 5 {
            xml.push_str(&format!(
                "<row r=\"{row}\"><c r=\"A{row}\"><v>{row}</v></c></row>"
            ));
        }
        xml.push_str("</sheetData></worksheet>");
        let (rows, truncated) = parse_sheet(xml.as_bytes(), &[], &Formats::default(), false, false);
        assert_eq!(rows.len(), MAX_ROWS_PER_SHEET);
        assert!(truncated);
        assert_eq!(rows[0], vec!["1"]);
    }

    #[test]
    fn cell_references_and_serial_dates_are_stable() {
        assert_eq!(cell_column("A1"), 0);
        assert_eq!(cell_column("Z9"), 25);
        assert_eq!(cell_column("AA1"), 26);
        assert_eq!(cell_column("AB12"), 27);
        assert_eq!(excel_date(45678.0, false, false), "2025-01-21");
        assert_eq!(excel_date(45678.5, true, false), "2025-01-21 12:00:00");
        assert_eq!(excel_date(1.0, false, false), "1900-01-01");
        assert_eq!(format_serial("0.125", CellFmt::Percent, false), "12.5%");
    }

    #[test]
    fn excel_dates_handle_1904_system_time_only_and_overflow() {
        assert_eq!(excel_date(0.0, false, true), "1904-01-01");
        assert_eq!(excel_date(45678.0, false, true), "2029-01-22");
        assert_eq!(excel_date(0.5, true, false), "12:00:00");
        assert_eq!(excel_date(0.999999999, true, false), "00:00:00");
        // Beyond Excel's last date the raw serial is shown instead of panicking.
        assert_eq!(excel_date(2_958_466.0, false, false), "2958466");
    }

    #[test]
    fn docx_decodes_entities_and_skips_drawing_text() {
        let xml = br#"<w:document xmlns:w="w"><w:body>
            <w:p><w:r><w:t>A &amp; B &#65;</w:t></w:r></w:p>
            <w:p><w:r><w:t>Body</w:t><w:drawing><a:p><a:t>Chart</a:t></a:p></w:drawing></w:r></w:p>
        </w:body></w:document>"#;
        let doc = parse_docx(xml, HashMap::new(), false);
        assert_eq!(doc.blocks[0].text, "A & B A");
        assert_eq!(doc.blocks[1].text, "Body");
    }

    #[test]
    fn docx_uses_style_names_for_localized_heading_ids() {
        let styles = parse_heading_styles(
            br#"<w:styles xmlns:w="w"><w:style w:styleId="1"><w:name w:val="heading 1"/></w:style>
            <w:style w:styleId="Custom"><w:name w:val="Title"/></w:style></w:styles>"#,
        );
        assert_eq!(styles.get("1"), Some(&1));
        assert_eq!(styles.get("Custom"), Some(&1));
        let xml = br#"<w:document xmlns:w="w"><w:body><w:p><w:pPr><w:pStyle w:val="1"/></w:pPr><w:r><w:t>H</w:t></w:r></w:p></w:body></w:document>"#;
        let doc = parse_docx(xml, styles, false);
        assert_eq!(doc.blocks[0].kind, "heading");
        assert_eq!(doc.blocks[0].level, Some(1));
    }

    #[test]
    fn sheet_length_caps_are_marked_truncated() {
        let xml = format!(
            "<worksheet><sheetData><row r=\"1\"><c r=\"A1\"><v>{}</v></c></row></sheetData></worksheet>",
            "1".repeat(MAX_CELL_BYTES + 10)
        );
        let (rows, truncated) = parse_sheet(xml.as_bytes(), &[], &Formats::default(), false, false);
        assert!(truncated);
        assert_eq!(rows[0][0].len(), MAX_CELL_BYTES);
    }

    #[test]
    fn normalize_target_resolves_relative_and_absolute_paths() {
        assert_eq!(normalize_target("worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(normalize_target("/xl/worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(normalize_target("../xl/worksheets/a.xml"), "xl/worksheets/a.xml");
        assert_eq!(normalize_target("xl/worksheets/a.xml"), "xl/worksheets/a.xml");
    }

    #[test]
    fn docx_reports_missing_part() {
        let mut archive = archive_from_parts(&[("other.xml", b"<x/>")]);
        // The public reader opens a path; here we only assert the part lookup fails.
        assert!(read_entry(&mut archive, "word/document.xml", MAX_XML_BYTES).is_none());
    }
}
