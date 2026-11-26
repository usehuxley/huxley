//! Exporting dashboards (ADR-052): each widget's answer as a table, in CSV
//! (blocks one after another), XLSX (a sheet per widget) or PDF (tables on
//! A4 pages, in the PDF's built-in Helvetica). Written here, without
//! further dependencies.

use std::fmt::Write as _;
use std::io::Write as _;

use serde_json::Value as Json;

use crate::Answer;

/// A widget's numbers, ready to write out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    /// Why there are no numbers (e.g. "Not available to you"), or that the
    /// rows were cut short.
    pub note: Option<String>,
}

/// A value: numbers stay numbers in a spreadsheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Text(String),
    /// Kept as written, e.g. "12.5".
    Number(String),
    Empty,
}

impl Cell {
    fn text(&self) -> &str {
        match self {
            Self::Text(s) | Self::Number(s) => s,
            Self::Empty => "",
        }
    }
}

fn cell(v: &Json) -> Cell {
    match v {
        Json::Null => Cell::Empty,
        Json::Number(n) => match n.as_f64() {
            Some(f) if f.fract() != 0.0 => Cell::Number(
                format!("{f:.2}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned(),
            ),
            _ => Cell::Number(n.to_string()),
        },
        Json::Bool(b) => Cell::Text(if *b { "Yes".into() } else { "No".into() }),
        Json::String(s) => Cell::Text(s.clone()),
        other => Cell::Text(other.to_string()),
    }
}

impl Table {
    /// A widget's answer as a table.
    pub fn from_answer(title: &str, answer: &Answer) -> Self {
        Self {
            title: title.to_owned(),
            columns: answer.columns.iter().map(|c| c.label.clone()).collect(),
            rows: answer
                .rows
                .iter()
                .map(|r| r.iter().map(cell).collect())
                .collect(),
            note: answer
                .truncated
                .then(|| "More rows matched than are shown.".to_owned()),
        }
    }

    /// A widget without numbers, saying why.
    pub fn unavailable(title: &str, why: &str) -> Self {
        Self {
            title: title.to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
            note: Some(why.to_owned()),
        }
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.starts_with(['=', '+', '-', '@']) {
        // Quoted, and formula-looking text defused for spreadsheets.
        let s = if s.starts_with(['=', '+', '-', '@']) && s.parse::<f64>().is_err() {
            format!("'{s}")
        } else {
            s.to_owned()
        };
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// The tables as CSV: a title line, a header and rows for each, a blank
/// line between.
pub fn csv(heading: &str, tables: &[Table]) -> Vec<u8> {
    let mut out = format!("{}\r\n\r\n", csv_field(heading));
    for (i, t) in tables.iter().enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        let _ = write!(out, "{}\r\n", csv_field(&t.title));
        if let Some(note) = &t.note {
            let _ = write!(out, "{}\r\n", csv_field(note));
        }
        if !t.columns.is_empty() {
            let _ = write!(
                out,
                "{}\r\n",
                t.columns
                    .iter()
                    .map(|c| csv_field(c))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        for r in &t.rows {
            let _ = write!(
                out,
                "{}\r\n",
                r.iter()
                    .map(|c| csv_field(c.text()))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    out.into_bytes()
}

fn xml(s: &str) -> String {
    s.chars()
        .filter(|c| matches!(*c, '\t' | '\n' | '\r') || *c >= ' ')
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A sheet name Excel accepts: at most 31 characters, none of `[]:*?/\`,
/// and unique.
fn sheet_name(title: &str, taken: &mut Vec<String>) -> String {
    let clean: String = title.chars().filter(|c| !"[]:*?/\\".contains(*c)).collect();
    let base: String = clean.trim().chars().take(28).collect();
    let base = if base.is_empty() {
        "Sheet".to_owned()
    } else {
        base
    };
    let mut name = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("{base} {n}");
        n += 1;
    }
    taken.push(name.clone());
    name
}

fn column(i: usize) -> String {
    let mut n = i + 1;
    let mut s = String::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.insert(0, char::from(b'A' + u8::try_from(r).unwrap_or(0)));
        n = (n - 1) / 26;
    }
    s
}

fn sheet_xml(t: &Table) -> String {
    let mut rows = String::new();
    let mut r = 1;
    let mut line = |cells: Vec<(String, bool)>, rows: &mut String| {
        let _ = write!(rows, "<row r=\"{r}\">");
        for (i, (v, number)) in cells.into_iter().enumerate() {
            let at = format!("{}{r}", column(i));
            if number {
                let _ = write!(rows, "<c r=\"{at}\"><v>{}</v></c>", xml(&v));
            } else if !v.is_empty() {
                let _ = write!(
                    rows,
                    "<c r=\"{at}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    xml(&v)
                );
            }
        }
        rows.push_str("</row>");
        r += 1;
    };
    line(vec![(t.title.clone(), false)], &mut rows);
    if let Some(note) = &t.note {
        line(vec![(note.clone(), false)], &mut rows);
    }
    if !t.columns.is_empty() {
        line(
            t.columns.iter().map(|c| (c.clone(), false)).collect(),
            &mut rows,
        );
    }
    for row in &t.rows {
        line(
            row.iter()
                .map(|c| (c.text().to_owned(), matches!(c, Cell::Number(_))))
                .collect(),
            &mut rows,
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>"
    )
}

/// The tables as an XLSX workbook, a sheet each.
pub fn xlsx(tables: &[Table]) -> std::io::Result<Vec<u8>> {
    let mut taken = Vec::new();
    let names: Vec<String> = tables
        .iter()
        .map(|t| sheet_name(&t.title, &mut taken))
        .collect();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, body: &str| -> std::io::Result<()> {
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        zip.write_all(body.as_bytes())
    };
    let overrides = (1..=tables.len()).fold(String::new(), |mut acc, i| {
        let _ = write!(acc, "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>");
        acc
    });
    put(
        "[Content_Types].xml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
             <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
             {overrides}</Types>"
        ),
    )?;
    put(
        "_rels/.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
         </Relationships>",
    )?;
    let sheets = names
        .iter()
        .enumerate()
        .fold(String::new(), |mut acc, (i, n)| {
            let _ = write!(
                acc,
                "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                xml(n),
                i + 1,
                i + 1
            );
            acc
        });
    put(
        "xl/workbook.xml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>{sheets}</sheets></workbook>"
        ),
    )?;
    let mut rels = (1..=tables.len()).fold(String::new(), |mut acc, i| {
        let _ = write!(acc, "<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>");
        acc
    });
    let _ = write!(
        rels,
        "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
        tables.len() + 1
    );
    put(
        "xl/_rels/workbook.xml.rels",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{rels}</Relationships>"
        ),
    )?;
    put(
        "xl/styles.xml",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
         <fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
         <borders count=\"1\"><border/></borders>\
         <cellStyleXfs count=\"1\"><xf/></cellStyleXfs><cellXfs count=\"1\"><xf/></cellXfs></styleSheet>",
    )?;
    for (i, t) in tables.iter().enumerate() {
        put(&format!("xl/worksheets/sheet{}.xml", i + 1), &sheet_xml(t))?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

/// Text in the PDF's `WinAnsi` encoding: Latin-1 as is, anything else `?`.
fn pdf_text(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let b = match c {
            '(' | ')' | '\\' => {
                out.push(b'\\');
                u8::try_from(u32::from(c)).unwrap_or(b'?')
            }
            '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => u8::try_from(u32::from(c)).unwrap_or(b'?'),
            '\u{2019}' | '\u{2018}' => b'\'',
            '\u{201c}' | '\u{201d}' => b'"',
            '\u{2013}' | '\u{2014}' => b'-',
            '\u{2026}' => b'.',
            _ => b'?',
        };
        out.push(b);
    }
    out
}

/// Cuts `s` to fit `width` points at `size` (Helvetica, roughly 0.5em a
/// character).
fn fit(s: &str, width: f64, size: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let max = ((width / (size * 0.5)).floor() as usize).max(1);
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!(
            "{}...",
            s.chars().take(max.saturating_sub(3)).collect::<String>()
        )
    }
}

const PAGE_W: f64 = 595.0;
const PAGE_H: f64 = 842.0;
const MARGIN: f64 = 40.0;

/// Lines laid out on pages: (font, size, x, y, text).
type Page = Vec<(u8, f64, f64, f64, String)>;

/// Lays text out down A4 pages.
struct Layout {
    pages: Vec<Page>,
    y: f64,
}

impl Layout {
    /// Text at `x`, `gap` below the last line (0: on the same line), on a
    /// new page when this one is full.
    fn line(&mut self, font: u8, size: f64, x: f64, text: String, gap: f64) {
        if self.y - gap < MARGIN {
            self.pages.push(Vec::new());
            self.y = PAGE_H - MARGIN;
        }
        self.y -= gap;
        let y = self.y;
        self.pages
            .last_mut()
            .expect("a page")
            .push((font, size, x, y, text));
    }

    /// A table row: each cell in its column, cut to fit.
    fn row(&mut self, font: u8, cells: Vec<String>, col_w: f64) {
        for (i, text) in cells.into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let x = MARGIN + col_w * i as f64;
            self.line(
                font,
                9.0,
                x,
                fit(&text, col_w - 4.0, 9.0),
                if i == 0 { 13.0 } else { 0.0 },
            );
        }
    }
}

fn layout(heading: &str, subheading: &str, tables: &[Table]) -> Vec<Page> {
    let mut l = Layout {
        pages: vec![Vec::new()],
        y: PAGE_H - MARGIN,
    };
    let width = PAGE_W - 2.0 * MARGIN;
    l.line(2, 16.0, MARGIN, fit(heading, width, 16.0), 16.0);
    l.line(1, 9.0, MARGIN, fit(subheading, width, 9.0), 14.0);
    for t in tables {
        l.line(2, 12.0, MARGIN, fit(&t.title, width, 12.0), 26.0);
        if let Some(note) = &t.note {
            l.line(1, 9.0, MARGIN, fit(note, width, 9.0), 13.0);
        }
        #[allow(clippy::cast_precision_loss)]
        let col_w = width / t.columns.len().max(1) as f64;
        if !t.columns.is_empty() {
            l.row(2, t.columns.clone(), col_w);
        }
        for r in &t.rows {
            l.row(1, r.iter().map(|c| c.text().to_owned()).collect(), col_w);
        }
    }
    l.pages
}

/// The tables as a PDF: a heading, a subheading (e.g. the range), then
/// each table, on as many A4 pages as it takes.
pub fn pdf(heading: &str, subheading: &str, tables: &[Table]) -> Vec<u8> {
    let pages = layout(heading, subheading, tables);
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    // 1 catalog, 2 pages, 3 Helvetica, 4 Helvetica-Bold, then a page and
    // its content for each page.
    let kids = (0..pages.len()).fold(String::new(), |mut acc, i| {
        let _ = write!(acc, "{} 0 R ", 5 + i * 2);
        acc
    });
    object(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(
        &mut out,
        &mut offsets,
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).as_bytes(),
    );
    object(
        &mut out,
        &mut offsets,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    object(
        &mut out,
        &mut offsets,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
    );
    for (i, page) in pages.iter().enumerate() {
        let mut content: Vec<u8> = Vec::new();
        for (font, size, x, y, text) in page {
            content
                .extend_from_slice(format!("BT /F{font} {size} Tf {x:.1} {y:.1} Td (").as_bytes());
            content.extend_from_slice(&pdf_text(text));
            content.extend_from_slice(b") Tj ET\n");
        }
        object(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                6 + i * 2
            )
            .as_bytes(),
        );
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(&content);
        stream.extend_from_slice(b"\nendstream");
        object(&mut out, &mut offsets, &stream);
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;

    fn tables() -> Vec<Table> {
        vec![
            Table {
                title: "Open by queue".into(),
                columns: vec!["Queue".into(), "Tickets".into()],
                rows: vec![
                    vec![
                        Cell::Text("Network, \"core\"".into()),
                        Cell::Number("12".into()),
                    ],
                    vec![
                        Cell::Text("=HYPERLINK(1)".into()),
                        Cell::Number("3.5".into()),
                    ],
                ],
                note: None,
            },
            Table::unavailable("Revenue", "Not available to you."),
        ]
    }

    #[test]
    fn csv_quotes_and_defuses_formulas() {
        let text = String::from_utf8(csv("Service desk", &tables())).unwrap();
        assert_eq!(
            text,
            "Service desk\r\n\r\nOpen by queue\r\nQueue,Tickets\r\n\"Network, \"\"core\"\"\",12\r\n\"'=HYPERLINK(1)\",3.5\r\n\r\nRevenue\r\nNot available to you.\r\n"
        );
    }

    #[test]
    fn xlsx_has_a_sheet_per_table_with_numbers_as_numbers() {
        let bytes = xlsx(&tables()).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut workbook = String::new();
        zip.by_name("xl/workbook.xml")
            .unwrap()
            .read_to_string(&mut workbook)
            .unwrap();
        assert!(
            workbook.contains("name=\"Open by queue\"") && workbook.contains("name=\"Revenue\"")
        );
        let mut sheet = String::new();
        zip.by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        assert!(sheet.contains("<c r=\"B3\"><v>12</v></c>"), "{sheet}");
        assert!(sheet.contains("Network, &quot;core&quot;"), "{sheet}");
    }

    #[test]
    fn pdf_is_a_pdf_with_its_pages() {
        let many = vec![Table {
            title: "Long".into(),
            columns: vec!["N".into()],
            rows: (0..200)
                .map(|i| vec![Cell::Number(i.to_string())])
                .collect(),
            note: None,
        }];
        let bytes = pdf("Report (café)", "Last 30 days", &many);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4") && text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Count 4 "), "200 rows take four pages");
        assert!(
            bytes.windows(13).any(|w| w == b"(Report \\(caf"),
            "parentheses escaped"
        );
        assert_eq!(sheet_name("Bad/Name:[x]", &mut vec![]), "BadNamex");
        assert_eq!(column(27), "AB");
    }
}

/// A file format to export to.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
#[schema(as = ExportFormat2)]
pub enum Format {
    Csv,
    Xlsx,
    Pdf,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "csv" => Some(Self::Csv),
            "xlsx" => Some(Self::Xlsx),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv; charset=utf-8",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Pdf => "application/pdf",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
            Self::Pdf => "pdf",
        }
    }
}

/// A range's words, e.g. `last_30_days` → "Last 30 days".
pub fn range_label(range: &str) -> String {
    let words = range.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// A file name from a dashboard's name: lower case, dashes, the extension.
pub fn file_name(name: &str, format: Format) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!(
        "{}.{}",
        if slug.is_empty() { "report" } else { &slug },
        format.extension()
    )
}

/// The distinct sources a dashboard's widgets read.
pub fn sources(dashboard: &crate::dashboards::Dashboard) -> Vec<String> {
    let mut keys: Vec<String> = dashboard
        .widgets
        .iter()
        .map(|w| w.spec.source.clone())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Each source's resolution: where the reader may see it, or why not.
pub type Resolved =
    std::collections::BTreeMap<String, Result<(crate::Source, crate::Scope), String>>;

/// Each widget of `dashboard` over `range`, as a table, with its source as
/// `resolved` says (see [`sources`]).
pub async fn tables(
    pool: &sqlx::PgPool,
    cache: &crate::Cache,
    dashboard: &crate::dashboards::Dashboard,
    range: &str,
    resolved: &Resolved,
) -> Vec<Table> {
    let mut out = Vec::with_capacity(dashboard.widgets.len());
    for w in &dashboard.widgets {
        let spec = crate::dashboards::with_range(w, range);
        let table = match resolved.get(&spec.source) {
            None => Table::unavailable(&w.title, "Not available to you."),
            Some(Err(why)) => Table::unavailable(&w.title, why),
            Some(Ok((source, scope))) => {
                match crate::run_for(pool, cache, source, &spec, scope).await {
                    Ok(answer) => Table::from_answer(&w.title, &answer),
                    Err(e) => Table::unavailable(&w.title, &format!("Couldn't load this: {e}")),
                }
            }
        };
        out.push(table);
    }
    out
}

/// The tables written out as `format`, headed by the dashboard's name and
/// the range.
pub fn render(
    format: Format,
    heading: &str,
    range: &str,
    tables: &[Table],
) -> std::io::Result<Vec<u8>> {
    let sub = range_label(range);
    match format {
        Format::Csv => Ok(csv(&format!("{heading} ({sub})"), tables)),
        Format::Xlsx => xlsx(tables),
        Format::Pdf => Ok(pdf(heading, &sub, tables)),
    }
}
