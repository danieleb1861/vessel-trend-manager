//! Native GUI: bounded plots are prepared once; SQLite never runs in the UI loop.
use crate::{
    Options,
    archive::{
        self, Catalog, Job, Loaded, MAX_BUCKETS, MAX_RAW, MAX_TAGS, Manager, Mode, Raw, Request,
        Sample, Trends,
    },
    set_italian, t,
};
use chrono::{DateTime, Duration as Span};
use eframe::egui::{self, Color32, RichText, Stroke};
use egui_plot::{Line, Plot, Points, Polygon};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

const TEAL: Color32 = Color32::from_rgb(0, 119, 128);
const INK: Color32 = Color32::from_rgb(24, 45, 62);
const MUTED: Color32 = Color32::from_rgb(91, 110, 126);
const COLORS: [Color32; 4] = [
    TEAL,
    Color32::from_rgb(57, 103, 184),
    Color32::from_rgb(148, 84, 179),
    Color32::from_rgb(183, 108, 35),
];
/// Quick periods: (English, Italian, days back from the archive end).
const PERIODS: [(&str, &str, Option<i64>); 8] = [
    ("24 hours", "24 ore", Some(1)),
    ("7 days", "7 giorni", Some(7)),
    ("30 days", "30 giorni", Some(30)),
    ("90 days", "90 giorni", Some(90)),
    ("180 days", "180 giorni", Some(180)),
    ("365 days", "365 giorni", Some(365)),
    ("Full history", "Tutto lo storico", None),
    ("Custom", "Personalizzato", None),
];
const FORMAT: &str = "%Y-%m-%d %H:%M";

#[derive(Clone, Serialize, Deserialize)]
struct SavedView {
    tags: Vec<String>,
    sorgente: Mode,
    secondi: f64,
    #[serde(default)]
    tutto_storico: bool,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    cartella: String,
    viste: BTreeMap<String, SavedView>,
    /// "it" for Italian; anything else means English.
    language: String,
    /// Machinery operating limits file imported by the user (local, never published).
    limiti: String,
}
/// Machinery operating limit of one signal, from a `tag;min;max;unit;notes` file.
struct Limit {
    min: Option<f64>,
    max: Option<f64>,
    unit: String,
}
impl Limit {
    fn matches_unit(&self, unit: &str) -> bool {
        // SI prefixes are case-sensitive: mW and MW differ by a factor of a billion.
        self.unit.is_empty() || self.unit == unit
    }
}
fn read_small_file(path: &Path) -> std::io::Result<Vec<u8>> {
    const MAX_BYTES: u64 = 1_048_576;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::other("File exceeds 1 MB"));
    }
    Ok(bytes)
}
fn write_settings(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let temp = path.with_file_name(format!("preferenze.{}.{stamp}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    // Replace the directory entry, never write through an existing preferences link.
    fs::rename(temp, path)
}
fn save_screenshot(path: &Path, bytes: &[u8], width: u32, height: u32) -> Result<(), String> {
    use image::ImageEncoder;
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(t("Choose a new .png file.", "Scegliere un nuovo file .png.").into());
    }
    // CREATE_NEW refuses existing files, including links: never overwrite an archive or image.
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            tr!(
                "Choose a new PNG filename: {}",
                "Scegliere un nuovo nome PNG: {}",
                e
            )
        })?;
    image::codecs::png::PngEncoder::new(file)
        .write_image(bytes, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}
fn parse_limits(text: &str) -> Result<BTreeMap<String, Limit>, String> {
    let mut limits = BTreeMap::new();
    for (number, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let fields: Vec<&str> = line.split(';').map(str::trim).collect();
        let tag = fields[0];
        // Comments, the header and rows not yet mapped to a signal are skipped.
        if tag.is_empty() || tag.starts_with('#') || tag.eq_ignore_ascii_case("tag") {
            continue;
        }
        let value = |i: usize| match fields.get(i).copied().unwrap_or("") {
            "" | "/" => Ok(None),
            v => v
                .replace(',', ".")
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .map(Some)
                .ok_or_else(|| {
                    tr!(
                        "Machinery limits line {}: invalid number {}.",
                        "Limiti dei macchinari, riga {}: numero non valido {}.",
                        number + 1,
                        v
                    )
                }),
        };
        let (min, max) = (value(1)?, value(2)?);
        if min.zip(max).is_some_and(|(a, b)| a > b) {
            return Err(tr!(
                "Machinery limits line {}: minimum above maximum.",
                "Limiti dei macchinari, riga {}: minimo superiore al massimo.",
                number + 1
            ));
        }
        let unit = fields.get(3).copied().unwrap_or("").to_string();
        if (min.is_some() || max.is_some())
            && limits
                .insert(tag.to_string(), Limit { min, max, unit })
                .is_some()
        {
            return Err(tr!(
                "Machinery limits line {}: {} repeated.",
                "Limiti dei macchinari, riga {}: {} ripetuto.",
                number + 1,
                tag
            ));
        }
    }
    Ok(limits)
}
fn read_limits(path: &Path) -> Result<BTreeMap<String, Limit>, String> {
    let text = read_small_file(path)
        .ok()
        // Excel saves plain CSV in the Windows code page: Latin-1 keeps letters and the degree sign.
        .map(|v| {
            String::from_utf8(v)
                .unwrap_or_else(|e| e.into_bytes().into_iter().map(char::from).collect())
        })
        .ok_or(t(
            "Machinery limits file unreadable: text up to 1 MB.",
            "File dei limiti dei macchinari non leggibile: testo fino a 1 MB.",
        ))?;
    parse_limits(&text)
}
/// Native line segments from groups of kept `[x, value]` readings. A disconnected group (invalid
/// readings, or digital states not all kept) and daily gaps over 900 s break the line.
fn lines(
    groups: impl IntoIterator<Item = (Vec<[f64; 2]>, bool)>,
    digital: bool,
    daily: bool,
) -> Vec<Vec<[f64; 2]>> {
    // x holds whole microseconds, so the gap rule matches `num_seconds() > 900` exactly.
    let micros = |x: f64| (x * 1e6).round();
    let mut segments = Vec::new();
    let mut line = Vec::new();
    let mut prior: Option<[f64; 2]> = None;
    for (retained, disconnected) in groups {
        if disconnected {
            if line.len() > 1 {
                segments.push(std::mem::take(&mut line));
            } else {
                line.clear();
            }
            prior = None;
        }
        for p in retained {
            if let Some(last) = prior {
                if (daily && micros(p[0]) - micros(last[0]) >= 901e6) || disconnected {
                    if line.len() > 1 {
                        segments.push(std::mem::take(&mut line));
                    } else {
                        line.clear();
                    }
                } else if digital {
                    line.push([p[0], last[1]]);
                }
            }
            line.push(p);
            prior = Some(p);
        }
        if disconnected {
            if line.len() > 1 && !digital {
                segments.push(std::mem::take(&mut line));
            } else {
                line.clear();
            }
            prior = None;
        }
    }
    if line.len() > 1 {
        segments.push(line);
    }
    segments
}
struct CachedPlot {
    points: Vec<Sample>,
    segments: Vec<Vec<[f64; 2]>>,
    bars: Vec<Vec<[f64; 2]>>,
    /// Raw drawing of the last plot range, rebuilt only when the range or width changes.
    view: RefCell<Option<RawView>>,
}
impl CachedPlot {
    fn new(series: &archive::Series, mode: Mode) -> Self {
        let mut points = Vec::new();
        let mut groups = Vec::new();
        let mut bars = Vec::new();
        let digital = series.tag.kind == "Digital";
        for bucket in series.buckets.values() {
            let retained = bucket.points();
            let disconnected = bucket.bad > 0 || (digital && retained.len() < bucket.count);
            groups.push((
                retained.iter().map(|p| [p.x(), p.value]).collect(),
                disconnected,
            ));
            for p in retained {
                if mode == Mode::Weekly {
                    bars.push(vec![[p.x(), p.low], [p.x(), p.high]]);
                }
                points.push(p.clone());
            }
        }
        Self {
            points,
            segments: lines(groups, digital, mode == Mode::Daily),
            bars,
            view: RefCell::new(None),
        }
    }
}
/// Raw readings drawn for one x range and plot width: every reading when they fit, otherwise
/// the first, last, minimum and maximum real reading of each pixel column.
struct RawView {
    key: (f64, f64, usize),
    /// Indices into `Series::raw` of the valid readings drawn.
    points: Vec<usize>,
    segments: Vec<Vec<[f64; 2]>>,
    /// x of groups with invalid/overridden readings, alarms and inhibits.
    flags: [Vec<f64>; 3],
    summary: Summary,
}
/// Readings inside the shown x range; statistics use valid readings only.
#[derive(Debug)]
struct Summary {
    count: usize,
    valid: usize,
    min: f64,
    max: f64,
    mean: f64,
    /// Population standard deviation.
    std: f64,
    /// Digital signals: rises and falls between consecutive valid readings; an invalid
    /// reading breaks the chain. Counts only, no durations.
    changes: Option<(usize, usize)>,
}
impl Summary {
    fn new(raw: &[Raw], x0: f64, x1: f64, digital: bool) -> Self {
        let first = raw.partition_point(|r| r.x < x0);
        let end = raw.partition_point(|r| r.x <= x1).max(first);
        let mut s = Self {
            count: end - first,
            valid: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            mean: 0.0,
            std: 0.0,
            changes: None,
        };
        if digital {
            let (mut rises, mut falls, mut prior) = (0, 0, None);
            for r in &raw[first..end] {
                let value = r.good().then_some(r.value);
                if let (Some(a), Some(b)) = (prior, value) {
                    rises += usize::from(b > a);
                    falls += usize::from(b < a);
                }
                prior = value;
            }
            s.changes = Some((rises, falls));
        }
        // Welford's update keeps the variance accurate over millions of readings.
        let mut m2 = 0.0;
        for r in raw[first..end].iter().filter(|r| r.good()) {
            s.valid += 1;
            s.min = s.min.min(r.value);
            s.max = s.max.max(r.value);
            let delta = r.value - s.mean;
            s.mean += delta / s.valid as f64;
            m2 += delta * (r.value - s.mean);
        }
        if s.valid > 0 {
            s.std = (m2 / s.valid as f64).sqrt();
        }
        s
    }
}
impl RawView {
    fn new(raw: &[Raw], key: (f64, f64, usize), digital: bool) -> Self {
        let (x0, x1, width) = key;
        let mut view = Self {
            key,
            points: Vec::new(),
            segments: Vec::new(),
            flags: Default::default(),
            summary: Summary::new(raw, x0, x1, digital),
        };
        // Half a view of margin on each side hides the one-frame bounds lag while panning.
        let (lo, hi) = (x0 - (x1 - x0) / 2.0, x1 + (x1 - x0) / 2.0);
        if !(lo.is_finite() && hi.is_finite() && hi > lo) {
            return view;
        }
        let columns = width.max(1) * 2;
        // One reading beyond each edge keeps lines running off the frame.
        let first = raw.partition_point(|r| r.x < lo).saturating_sub(1);
        let last = (raw.partition_point(|r| r.x <= hi) + 1).min(raw.len());
        let every = last - first <= columns * 2;
        let column =
            |r: &Raw| (((r.x - lo) / (hi - lo) * columns as f64).floor() as usize).min(columns - 1);
        let mut groups = Vec::new();
        let mut members = Vec::new();
        let mut index = first;
        while index < last {
            let group = column(&raw[index]);
            members.clear();
            while index < last && (members.is_empty() || (!every && column(&raw[index]) == group)) {
                members.push(index);
                index += 1;
            }
            let good: Vec<usize> = members.iter().copied().filter(|&i| raw[i].good()).collect();
            let by_value = |a: &&usize, b: &&usize| raw[**a].value.total_cmp(&raw[**b].value);
            let mut kept: Vec<usize> = [
                good.first(),
                good.last(),
                good.iter().min_by(by_value),
                good.iter().max_by(by_value),
            ]
            .into_iter()
            .flatten()
            .copied()
            .collect();
            kept.sort_unstable();
            kept.dedup();
            let x = if every {
                raw[members[0]].x
            } else {
                lo + (group as f64 + 0.5) * (hi - lo) / columns as f64
            };
            for (bit, flags) in view.flags.iter_mut().enumerate() {
                if members.iter().any(|&i| raw[i].flags & (1 << bit) != 0) {
                    flags.push(x);
                }
            }
            let bad = good.len() < members.len();
            groups.push((
                kept.iter().map(|&i| [raw[i].x, raw[i].value]).collect(),
                bad || (digital && kept.len() < members.len()),
            ));
            view.points.extend(kept);
        }
        view.segments = lines(groups, digital, true);
        view
    }
}
fn settings_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("preferenze.json")))
        .unwrap_or_else(|| PathBuf::from("preferenze.json"))
}
fn time_label(value: f64, format: &str) -> String {
    if !value.is_finite() {
        return String::new();
    }
    DateTime::from_timestamp(value as i64, 0)
        .map(|t| t.format(format).to_string())
        .unwrap_or_default()
}
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}
/// Language flag button: Italian tricolour or a simplified Union Jack.
fn flag(ui: &mut egui::Ui, italian: bool, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 20.0), egui::Sense::click());
    let painter = ui.painter().with_clip_rect(rect);
    if italian {
        let width = rect.width() / 3.0;
        for (i, color) in [
            Color32::from_rgb(0, 146, 70),
            Color32::WHITE,
            Color32::from_rgb(206, 43, 55),
        ]
        .into_iter()
        .enumerate()
        {
            let min = rect.min + egui::vec2(width * i as f32, 0.0);
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(width, rect.height())),
                0.0,
                color,
            );
        }
    } else {
        let red = Color32::from_rgb(200, 16, 46);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(1, 33, 105));
        for line in [
            [rect.left_top(), rect.right_bottom()],
            [rect.right_top(), rect.left_bottom()],
        ] {
            painter.line_segment(line, Stroke::new(4.0_f32, Color32::WHITE));
            painter.line_segment(line, Stroke::new(1.5_f32, red));
        }
        let c = rect.center();
        for line in [
            [egui::pos2(rect.left(), c.y), egui::pos2(rect.right(), c.y)],
            [egui::pos2(c.x, rect.top()), egui::pos2(c.x, rect.bottom())],
        ] {
            painter.line_segment(line, Stroke::new(6.0_f32, Color32::WHITE));
            painter.line_segment(line, Stroke::new(3.5_f32, red));
        }
    }
    if !selected {
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(120));
    }
    let border = if selected {
        Stroke::new(2.0_f32, Color32::WHITE)
    } else {
        Stroke::new(1.0_f32, Color32::from_gray(120))
    };
    ui.painter()
        .rect_stroke(rect, 2.0, border, egui::StrokeKind::Outside);
    response.on_hover_text(if italian { "Italiano" } else { "English" })
}
fn date_grid(input: egui_plot::GridInput) -> Vec<egui_plot::GridMark> {
    let span = input.bounds.1 - input.bounds.0;
    if !span.is_finite() || span <= 0.0 {
        return Vec::new();
    }
    let steps = [
        1.0, 5.0, 15.0, 60.0, 300.0, 900.0, 3600.0, 21600.0, 86400.0, 604800.0, 2592000.0,
        31536000.0,
    ];
    let step = steps
        .into_iter()
        .find(|v| *v >= span / 7.0)
        .unwrap_or(span / 7.0);
    let first = (input.bounds.0 / step).ceil() * step;
    (0..9)
        .map(|i| first + i as f64 * step)
        .filter(|v| *v <= input.bounds.1)
        .map(|value| egui_plot::GridMark {
            value,
            step_size: step,
        })
        .collect()
}
pub struct App {
    manager: Manager,
    catalog: Option<Arc<Catalog>>,
    trends: Option<Arc<Trends>>,
    /// Last complete raw load from disk: shorter periods of its signals are served from it.
    /// Memory only; dropped when the archive folder is closed or reopened.
    cache: Option<Arc<Trends>>,
    plots: Vec<CachedPlot>,
    folder: String,
    search: String,
    group: String,
    kind: usize,
    selected: Vec<String>,
    mode: Mode,
    period: usize,
    start: String,
    end: String,
    busy: bool,
    status: String,
    error: Option<String>,
    settings: Settings,
    view_name: String,
    notes: bool,
    help: bool,
    reset: bool,
    options: Options,
    smoke_stage: u8,
    frames: u32,
    began: Instant,
    smoke_result: Arc<Mutex<Option<Result<(), String>>>>,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    /// Preselect the featured demo signals once the demo catalog arrives.
    featured: bool,
    /// Only an explicitly opened demo may supply synthetic limits automatically.
    demo_folder: Option<PathBuf>,
    limits: BTreeMap<String, Limit>,
    /// Active limits file and its modification time, checked for user edits.
    limits_file: Option<(PathBuf, Option<SystemTime>)>,
    limits_checked: Instant,
}
impl App {
    fn new(
        ctx: &egui::Context,
        options: Options,
        smoke_result: Arc<Mutex<Option<Result<(), String>>>>,
    ) -> Self {
        ctx.set_visuals(egui::Visuals::light());
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(13.0, 8.0);
        style.visuals.panel_fill = Color32::from_rgb(246, 248, 251);
        style.visuals.override_text_color = Some(INK);
        style.visuals.selection.bg_fill = Color32::from_rgb(210, 237, 237);
        style.visuals.selection.stroke = Stroke::new(1.0_f32, TEAL);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
        ctx.set_style(style);
        let mut warning = None;
        let settings = if options.smoke {
            Settings::default()
        } else {
            let path = settings_path();
            if path.exists() {
                let parsed = read_small_file(&path)
                    .ok()
                    .and_then(|v| serde_json::from_slice(&v).ok());
                if parsed.is_none() {
                    warning=Some("Preferences unreadable: starting with defaults. The original file was not modified. / Preferenze non leggibili: avvio con impostazioni iniziali. Il file originale non e stato modificato.".into());
                }
                parsed.unwrap_or_default()
            } else {
                Settings::default()
            }
        };
        set_italian(settings.language == "it");
        let folder = options
            .folder
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| settings.cartella.clone());
        let mode = if options.weekly {
            Mode::Weekly
        } else {
            Mode::Daily
        };
        let demo_folder = options.demo.then(|| PathBuf::from(&folder));
        let mut app = Self {
            manager: Manager::new(),
            catalog: None,
            trends: None,
            cache: None,
            plots: Vec::new(),
            folder,
            search: String::new(),
            group: String::new(),
            kind: 0,
            selected: Vec::new(),
            mode,
            period: 0,
            start: String::new(),
            end: String::new(),
            busy: false,
            status: t(
                "Choose an export folder to start",
                "Scegli una cartella di export per iniziare",
            )
            .into(),
            error: warning,
            settings,
            view_name: String::new(),
            notes: false,
            help: false,
            reset: false,
            screenshot: options.screenshot.clone(),
            screenshot_requested: false,
            featured: false,
            demo_folder,
            limits: BTreeMap::new(),
            limits_file: None,
            limits_checked: Instant::now(),
            options,
            smoke_stage: 0,
            frames: 0,
            began: Instant::now(),
            smoke_result,
        };
        if !app.folder.is_empty() {
            app.refresh();
        }
        app
    }
    fn refresh(&mut self) {
        self.open(Job::Scan(PathBuf::from(&self.folder)));
    }
    /// Demo folders use their synthetic limits; otherwise the user's imported file.
    fn reload_limits(&mut self) {
        let demo = Path::new(&self.folder).join(crate::demo::LIMITS);
        if self.demo_folder.as_deref() == Some(Path::new(&self.folder)) {
            self.read_limits_file(demo);
        } else if self.settings.limiti.is_empty() {
            self.limits.clear();
            self.limits_file = None;
        } else {
            self.read_limits_file(PathBuf::from(&self.settings.limiti));
        }
    }
    /// Reads the active limits file; an unreadable update must not leave stale limits on screen.
    fn read_limits_file(&mut self, path: PathBuf) -> bool {
        let stamp = fs::metadata(&path).and_then(|m| m.modified()).ok();
        let result = read_limits(&path);
        self.limits_file = Some((path, stamp));
        match result {
            Ok(limits) => {
                self.limits = limits;
                true
            }
            Err(e) => {
                self.limits.clear();
                self.error = Some(e);
                false
            }
        }
    }
    fn import_limits(&mut self, path: PathBuf) {
        if self.read_limits_file(path.clone()) {
            self.status = tr!(
                "{} machinery operating limits imported",
                "{} limiti dei macchinari importati",
                self.limits.len()
            );
            self.settings.limiti = path.to_string_lossy().into_owned();
            self.save_settings();
        }
    }
    fn open(&mut self, job: Job) {
        match &job {
            Job::Demo(folder) => self.demo_folder = Some(folder.clone()),
            Job::Scan(folder) if self.demo_folder.as_ref() != Some(folder) => {
                self.demo_folder = None
            }
            _ => {}
        }
        self.catalog = None;
        self.selected.clear();
        self.trends = None;
        self.cache = None;
        self.plots.clear();
        self.group.clear();
        self.error = None;
        self.busy = true;
        self.status = t("Reading catalog...", "Lettura catalogo...").into();
        self.manager.submit(job);
    }
    fn set_period(&mut self) {
        if self.period == 7 {
            return;
        }
        if let Some(c) = &self.catalog {
            match c.bounds(self.mode) {
                Ok((first, end)) => {
                    let start = PERIODS[self.period]
                        .2
                        .and_then(|days| end.checked_sub_signed(Span::days(days)))
                        .unwrap_or(first);
                    self.start = start.format(FORMAT).to_string();
                    self.end = end.format(FORMAT).to_string();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }
    fn load(&mut self) {
        let Some(catalog) = &self.catalog else {
            self.error = Some(t("Wait for the catalog.", "Attendere il catalogo.").into());
            return;
        };
        let request = (|| {
            let r = Request {
                tags: self.selected.clone(),
                start: crate::date(&self.start)?,
                end: crate::date(&self.end)?,
                mode: self.mode,
                buckets: MAX_BUCKETS,
            };
            r.validate()?;
            Ok::<_, String>(r)
        })();
        match request {
            Ok(r) => {
                if let Some(t) = self.cache.as_ref().and_then(|c| archive::narrow(c, &r)) {
                    // Nothing to read: a running load is dropped and the result shown at once.
                    self.manager.supersede();
                    self.busy = false;
                    self.error = None;
                    self.show_trends(t, true);
                    return;
                }
                // A wider period of the cached signals reads only what memory lacks.
                self.manager
                    .submit(Job::Load(catalog.clone(), r, self.cache.clone()));
                self.busy = true;
                self.error = None;
                self.trends = None;
                self.plots.clear();
                self.status = t("Loading trends...", "Caricamento trend...").into();
            }
            Err(e) => self.error = Some(e),
        }
    }
    fn poll(&mut self) {
        // The limits file is meant to be edited: saved changes appear without a restart.
        if self.limits_checked.elapsed() > Duration::from_secs(2) {
            self.limits_checked = Instant::now();
            if let Some((path, stamp)) = self.limits_file.clone()
                && fs::metadata(&path).and_then(|m| m.modified()).ok() != stamp
            {
                self.read_limits_file(path);
            }
        }
        if let Some(p) = self.manager.progress() {
            self.status = p;
        }
        if let Some(event) = self.manager.poll() {
            self.busy = false;
            match event {
                Ok(Loaded::Catalog(c)) => {
                    self.status = tr!(
                        "Archive ready · {} files · {} signals",
                        "Archivio pronto · {} file · {} parametri",
                        c.archives.len(),
                        c.tags.len()
                    );
                    self.catalog = Some(c.clone());
                    self.reload_limits();
                    self.set_period();
                    if std::mem::take(&mut self.featured) {
                        self.selected = crate::demo::FEATURED
                            .iter()
                            .filter(|n| c.tags.contains_key(**n))
                            .map(|n| n.to_string())
                            .collect();
                        self.mode = Mode::Daily;
                        self.period = 1;
                        self.set_period();
                        self.load();
                    } else if self.options.smoke || self.options.screenshot.is_some() {
                        match crate::request(&c, &self.options) {
                            Ok(r) => {
                                self.selected = r.tags;
                                self.mode = r.mode;
                                self.start = r.start.format(FORMAT).to_string();
                                self.end = r.end.format(FORMAT).to_string();
                                self.period = 7;
                                self.smoke_stage = 1;
                                self.load();
                            }
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
                Ok(Loaded::Trends(t)) => self.show_trends(t, false),
                Err(e) => {
                    self.status = t("Operation finished", "Operazione terminata").into();
                    self.error = Some(e);
                }
            }
        }
    }
    /// Shows loaded trends; a complete raw load read from disk becomes the memory cache.
    fn show_trends(&mut self, t: Trends, from_memory: bool) {
        self.status = if from_memory {
            tr!(
                "{} readings · all raw, from memory · {:.2} s",
                "{} letture · tutte raw, dalla memoria · {:.2} s",
                t.series.iter().map(|s| s.count).sum::<usize>(),
                t.seconds
            )
        } else if t.raw && t.reused > 0 {
            tr!(
                "{} readings ({} from memory) · all raw, zoom for detail · {:.2} s",
                "{} letture ({} dalla memoria) · tutte raw, zoom per il dettaglio · {:.2} s",
                t.series.iter().map(|s| s.count).sum::<usize>(),
                t.reused,
                t.seconds
            )
        } else if t.raw {
            tr!(
                "{} readings · all raw, zoom for detail · {:.2} s",
                "{} letture · tutte raw, zoom per il dettaglio · {:.2} s",
                t.series.iter().map(|s| s.count).sum::<usize>(),
                t.seconds
            )
        } else {
            tr!(
                "{} readings · {} plotted points · {:.2} s",
                "{} letture · {} punti nel grafico · {:.2} s",
                t.series.iter().map(|s| s.count).sum::<usize>(),
                t.series
                    .iter()
                    .flat_map(|s| s.buckets.values())
                    .map(|b| b.points().len())
                    .sum::<usize>(),
                t.seconds
            )
        };
        self.plots = t
            .series
            .iter()
            .map(|s| CachedPlot::new(s, t.request.mode))
            .collect();
        let t = Arc::new(t);
        if t.raw && !from_memory {
            self.cache = Some(t.clone());
        }
        self.trends = Some(t);
        self.reset = true;
        self.smoke_stage = 2;
    }
    fn save_settings(&mut self) {
        if self.options.smoke {
            return;
        }
        self.settings.cartella = self.folder.clone();
        let result = (|| {
            let path = settings_path();
            let data = serde_json::to_vec_pretty(&self.settings).map_err(|e| e.to_string())?;
            write_settings(&path, &data).map_err(|e| e.to_string())
        })();
        if let Err(e) = result {
            self.error = Some(tr!(
                "Preferences not saved: {}",
                "Preferenze non salvate: {}",
                e
            ));
        }
    }
    fn save_view(&mut self) {
        let name = self.view_name.trim().chars().take(60).collect::<String>();
        let valid = (|| {
            let start = crate::date(&self.start)?;
            let end = crate::date(&self.end)?;
            Request {
                tags: self.selected.clone(),
                start,
                end,
                mode: self.mode,
                buckets: MAX_BUCKETS,
            }
            .validate()?;
            if name.is_empty() {
                return Err(t(
                    "Enter a name for the view.",
                    "Scrivere un nome per la vista.",
                )
                .into());
            }
            Ok((end - start).num_seconds() as f64)
        })();
        match valid {
            Ok(seconds) => {
                self.settings.viste.insert(
                    name,
                    SavedView {
                        tags: self.selected.clone(),
                        sorgente: self.mode,
                        secondi: seconds,
                        tutto_storico: self.period == 6,
                    },
                );
                self.save_settings();
            }
            Err(e) => self.error = Some(e),
        }
    }
    fn load_view(&mut self, name: &str) {
        let Some(c) = &self.catalog else {
            return;
        };
        let Some(v) = self.settings.viste.get(name).cloned() else {
            return;
        };
        if v.tags.is_empty()
            || v.tags.len() > MAX_TAGS
            || v.tags.iter().any(|n| !c.tags.contains_key(n))
            || !v.secondi.is_finite()
            || v.secondi <= 0.0
            || v.secondi > 31_536_000_000.0
        {
            self.error = Some(
                t(
                    "View not valid for this archive: create it again.",
                    "Vista non valida per questo archivio: ricrearla.",
                )
                .into(),
            );
            return;
        }
        match c.bounds(v.sorgente) {
            Ok((first, end)) => {
                let Some(start) = (if v.tutto_storico {
                    Some(first)
                } else {
                    end.checked_sub_signed(Span::seconds(v.secondi as i64))
                }) else {
                    self.error = Some(t("Invalid duration.", "Durata non valida.").into());
                    return;
                };
                self.mode = v.sorgente;
                self.period = if v.tutto_storico { 6 } else { 7 };
                self.selected = v.tags;
                self.start = start.format(FORMAT).to_string();
                self.end = end.format(FORMAT).to_string();
                self.view_name = name.into();
                self.load();
            }
            Err(e) => self.error = Some(e),
        }
    }
    fn top(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(INK).inner_margin(18))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("VESSEL")
                            .size(23.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.label(
                        RichText::new("TREND MANAGER")
                            .size(19.0)
                            .color(Color32::from_rgb(143, 211, 214)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(t("Guide", "Guida")).clicked() {
                            self.help = true;
                        }
                        let italian = crate::italian();
                        for flag_italian in [false, true] {
                            if flag(ui, flag_italian, italian == flag_italian).clicked()
                                && italian != flag_italian
                            {
                                set_italian(flag_italian);
                                self.settings.language =
                                    if flag_italian { "it" } else { "en" }.into();
                                self.save_settings();
                            }
                        }
                        ui.label(
                            RichText::new(t(
                                "OFFLINE  /  READ-ONLY ARCHIVES",
                                "OFFLINE  /  ARCHIVI IN SOLA LETTURA",
                            ))
                            .size(11.0)
                            .color(Color32::from_gray(210)),
                        );
                    });
                });
            });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if self.busy {
                    ui.spinner();
                } else {
                    dot(ui, TEAL);
                }
                ui.label(&self.status);
                if self.busy
                    && ui
                        .button(t("Cancel loading", "Annulla caricamento"))
                        .clicked()
                {
                    self.manager.cancel();
                    self.status = t("Cancelling...", "Annullamento...").into();
                }
            });
        });
    }
    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("signals")
            .default_width(300.0)
            .min_width(260.0)
            .max_width(440.0)
            .show(ctx, |ui| {
                let list_height = (ui.available_height() - 650.0).clamp(80.0, 420.0);
                egui::ScrollArea::vertical()
                    .id_salt("sidebar_all")
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        ui.heading(t("Archive and signals", "Archivio e parametri"));
                        ui.horizontal(|ui| {
                            if ui.button(t("Data folder...", "Cartella dati...")).clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .set_title(t("AMS export folder", "Cartella export AMS"))
                                    .pick_folder()
                            {
                                self.folder = path.to_string_lossy().into_owned();
                                self.refresh();
                                self.save_settings();
                            }
                            if ui
                                .button(t("Try demo", "Prova demo"))
                                .on_hover_text(t(
                                    "Open synthetic demo archives (no real data)",
                                    "Apri archivi demo sintetici (nessun dato reale)",
                                ))
                                .clicked()
                            {
                                let folder = crate::demo::default_folder();
                                self.folder = folder.to_string_lossy().into_owned();
                                self.featured = true;
                                self.open(Job::Demo(folder));
                            }
                            if ui
                                .add_enabled(
                                    !self.folder.is_empty(),
                                    egui::Button::new(t("Refresh", "Aggiorna")),
                                )
                                .clicked()
                            {
                                self.refresh();
                            }
                        });
                        ui.add(
                            egui::TextEdit::singleline(&mut self.folder)
                                .hint_text(t("Export folder path", "Percorso cartella export"))
                                .desired_width(f32::INFINITY),
                        );
                        if let Some(c) = &self.catalog {
                            ui.small(tr!(
                                "{} archives · {} signals",
                                "{} archivi · {} parametri",
                                c.archives.len(),
                                c.tags.len()
                            ));
                            for mode in [Mode::Daily, Mode::Weekly] {
                                if let Ok((a, b)) = c.bounds(mode) {
                                    ui.label(
                                        RichText::new(format!(
                                            "{}: {} - {}",
                                            mode.label(),
                                            a.format("%d/%m/%Y"),
                                            b.format("%d/%m/%Y")
                                        ))
                                        .size(11.0)
                                        .color(MUTED),
                                    );
                                }
                            }
                            if !c.warnings.is_empty()
                                && ui
                                    .button(tr!(
                                        "Archive notes ({})",
                                        "Note archivio ({})",
                                        c.warnings.len()
                                    ))
                                    .clicked()
                            {
                                self.notes = true;
                            }
                        }
                        ui.separator();
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text(t(
                                    "Search system, signal or tag...",
                                    "Cerca impianto, parametro o tag...",
                                ))
                                .desired_width(f32::INFINITY),
                        );
                        egui::ComboBox::from_id_salt("group")
                            .selected_text(if self.group.is_empty() {
                                t("All systems", "Tutti gli impianti")
                            } else {
                                &self.group
                            })
                            .width(ui.available_width() - 10.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.group,
                                    String::new(),
                                    t("All systems", "Tutti gli impianti"),
                                );
                                if let Some(c) = &self.catalog {
                                    let groups: std::collections::BTreeSet<_> =
                                        c.tags.values().map(|t| &t.group).collect();
                                    for group in groups {
                                        ui.selectable_value(&mut self.group, group.clone(), group);
                                    }
                                }
                            });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.kind, 0, t("Analog", "Analogici"));
                            ui.selectable_value(&mut self.kind, 1, t("Digital", "Digitali"));
                            ui.selectable_value(&mut self.kind, 2, t("All", "Tutti"));
                        });
                        let words: Vec<_> = self
                            .search
                            .to_lowercase()
                            .split_whitespace()
                            .map(str::to_owned)
                            .collect();
                        let signals: Vec<_> = self
                            .catalog
                            .as_ref()
                            .map(|c| {
                                c.tags
                                    .values()
                                    .filter(|t| {
                                        (self.group.is_empty() || self.group == t.group)
                                            && (self.kind == 2
                                                || t.kind
                                                    == if self.kind == 0 {
                                                        "Analog"
                                                    } else {
                                                        "Digital"
                                                    })
                                            && words.iter().all(|w| {
                                                format!("{} {} {}", t.name, t.description, t.group)
                                                    .to_lowercase()
                                                    .contains(w)
                                            })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        ui.small(tr!(
                            "{} results · choose up to four signals",
                            "{} risultati · scegli fino a quattro parametri",
                            signals.len()
                        ));
                        egui::ScrollArea::vertical()
                            .id_salt("tag_list")
                            .max_height(list_height)
                            .show_rows(ui, 42.0, signals.len(), |ui, range| {
                                for index in range {
                                    let tag = signals[index];
                                    let mut checked = self.selected.contains(&tag.name);
                                    let response = ui.add_enabled(
                                        checked || self.selected.len() < MAX_TAGS,
                                        egui::Checkbox::new(
                                            &mut checked,
                                            format!(
                                                "{}\n{}  ·  {}",
                                                tag.description, tag.name, tag.unit
                                            ),
                                        ),
                                    );
                                    if response.changed() {
                                        if checked {
                                            self.selected.push(tag.name.clone());
                                        } else {
                                            self.selected.retain(|n| n != &tag.name);
                                        }
                                    }
                                }
                            });
                        ui.separator();
                        ui.label(
                            RichText::new(tr!(
                                "SELECTED  {}/4",
                                "SELEZIONATI  {}/4",
                                self.selected.len()
                            ))
                            .size(11.0)
                            .strong()
                            .color(MUTED),
                        );
                        let mut remove = None;
                        for (i, name) in self.selected.iter().enumerate() {
                            ui.horizontal(|ui| {
                                dot(ui, COLORS[i]);
                                ui.label(name);
                                if ui.small_button(t("Remove", "Rimuovi")).clicked() {
                                    remove = Some(name.clone());
                                }
                            });
                        }
                        if let Some(name) = remove {
                            self.selected.retain(|n| n != &name);
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(t("SAVED VIEWS", "VISTE ABITUALI"))
                                .size(11.0)
                                .strong()
                                .color(MUTED),
                        );
                        let mut chosen = None;
                        egui::ComboBox::from_id_salt("views")
                            .selected_text(t("Open a view...", "Richiama una vista..."))
                            .width(ui.available_width() - 10.0)
                            .show_ui(ui, |ui| {
                                for name in self.settings.viste.keys() {
                                    if ui.selectable_label(false, name).clicked() {
                                        chosen = Some(name.clone());
                                    }
                                }
                            });
                        if let Some(name) = chosen {
                            self.load_view(&name);
                        }
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.view_name)
                                    .hint_text(t("View name", "Nome della vista"))
                                    .desired_width(160.0),
                            );
                            if ui.button(t("Save", "Salva")).clicked() {
                                self.save_view();
                            }
                        });
                    });
            });
    }
    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(t("Machinery trends", "Trend dei macchinari"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.trends.is_some(),
                        egui::Button::new(t("Save screenshot", "Salva schermata")),
                    )
                    .clicked()
                {
                    self.screenshot = rfd::FileDialog::new()
                        .add_filter(t("PNG image", "Immagine PNG"), &["png"])
                        .set_file_name("trend.png")
                        .save_file();
                    self.screenshot_requested = false;
                }
                if ui.button(t("Reset zoom", "Ripristina zoom")).clicked() {
                    self.reset = true;
                }
                if ui
                    .button(t("Machinery limits...", "Limiti macchinari..."))
                    .on_hover_text(t(
                        "Import machinery operating limits (CSV, editable in Excel: tag;min;max;unit)",
                        "Importa i limiti dei macchinari (CSV, modificabile in Excel: tag;min;max;unita)",
                    ))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV", &["csv", "txt"])
                        .pick_file()
                {
                    self.import_limits(path);
                }
            });
        });
        egui::Frame::new().fill(Color32::WHITE).corner_radius(8).inner_margin(14).show(ui,|ui| {
            ui.horizontal_wrapped(|ui| {
                let old=self.mode;
                ui.selectable_value(&mut self.mode,Mode::Daily,Mode::Daily.label());ui.selectable_value(&mut self.mode,Mode::Weekly,Mode::Weekly.label());
                if self.mode!=old {self.set_period();}
                ui.separator();
                let old=self.period;
                egui::ComboBox::from_id_salt("period").selected_text(t(PERIODS[self.period].0,PERIODS[self.period].1)).width(145.0).show_ui(ui,|ui| {
                    for (i,(en,it,_)) in PERIODS.iter().enumerate(){ui.selectable_value(&mut self.period,i,t(en,it));}
                });
                if self.period!=old {self.set_period();}
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(t("From","Da"));if ui.add(egui::TextEdit::singleline(&mut self.start).desired_width(150.0).hint_text(t("YYYY-MM-DD HH:MM","AAAA-MM-GG HH:MM"))).changed(){self.period=7;}
                ui.label(t("To","A"));if ui.add(egui::TextEdit::singleline(&mut self.end).desired_width(150.0).hint_text(t("YYYY-MM-DD HH:MM","AAAA-MM-GG HH:MM"))).changed(){self.period=7;}
                if ui.add_enabled(self.catalog.is_some() && !self.selected.is_empty(),egui::Button::new(RichText::new(t("Load trends","Carica trend")).color(Color32::WHITE)).fill(TEAL)).clicked(){self.load();}
            });
            ui.label(RichText::new(t("Original archive time · end excluded · no live connection","Orario originale dell'archivio · fine esclusa · nessun collegamento in tempo reale")).size(11.0).color(MUTED));
        });
        if let Some(error) = self.error.clone() {
            egui::Frame::new()
                .fill(Color32::from_rgb(255, 237, 221))
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(Color32::from_rgb(128, 57, 18), error);
                        if ui.small_button(t("Close", "Chiudi")).clicked() {
                            self.error = None;
                        }
                    });
                });
        }
    }
    fn charts(&mut self, ui: &mut egui::Ui) {
        let Some(trends) = &self.trends else {
            ui.add_space(70.0);
            ui.vertical_centered(|ui| {
                ui.heading(if self.busy {
                    t("Preparing trends", "Preparazione dei trend")
                } else {
                    t(
                        "The right data, over the period you choose",
                        "I dati giusti, nel periodo che scegli",
                    )
                });
                ui.add_space(12.0);
                ui.label(t(
                    "1. Open the archives     2. Select the signals     3. Load the period",
                    "1. Apri gli archivi     2. Seleziona i parametri     3. Carica il periodo",
                ));
                if self.busy {
                    ui.spinner();
                } else {
                    ui.colored_label(
                        MUTED,
                        t(
                            "You can also browse the full available history.",
                            "Puoi consultare anche tutto lo storico disponibile.",
                        ),
                    );
                }
            });
            return;
        };
        ui.label(
            RichText::new(tr!(
                "SHOWN: {}  -  {}  |  {}",
                "VISUALIZZATO: {}  -  {}  |  {}",
                trends.request.start.format(FORMAT),
                trends.request.end.format(FORMAT),
                trends.request.mode.label()
            ))
            .size(12.0)
            .strong()
            .color(TEAL),
        );
        if self.selected != trends.request.tags
            || self.start != trends.request.start.format(FORMAT).to_string()
            || self.end != trends.request.end.format(FORMAT).to_string()
            || self.mode != trends.request.mode
        {
            ui.colored_label(
                Color32::from_rgb(155, 94, 20),
                t(
                    "Selection changed: press Load trends to apply it.",
                    "Selezione modificata: premi Carica trend per applicarla.",
                ),
            );
        }
        if trends.request.mode == Mode::Daily && !trends.raw {
            ui.colored_label(
                Color32::from_rgb(155, 94, 20),
                tr!(
                    "Reduced view: over {} million readings. Shorten the period or reduce the signals to see every raw reading.",
                    "Vista ridotta: oltre {} milioni di letture. Ridurre il periodo o i parametri per vedere tutte le letture raw.",
                    MAX_RAW / 1_000_000
                ),
            );
        }
        let plot_height = if trends.series.len() <= 2 {
            ((ui.available_height() - 80.0) / trends.series.len().max(1) as f32 - 90.0).max(190.0)
        } else {
            175.0
        };
        let reset = self.reset;
        egui::ScrollArea::vertical().id_salt("charts").show(ui,|ui| {
            for (index,(series,cache)) in trends.series.iter().zip(&self.plots).enumerate() {
                egui::Frame::new().fill(Color32::WHITE).corner_radius(8).inner_margin(12).show(ui,|ui| {
                    ui.horizontal_wrapped(|ui|{dot(ui,COLORS[index]);ui.label(RichText::new(&series.tag.description).size(17.0).strong());ui.label(RichText::new(format!("{}  /  {}",series.tag.name,series.tag.unit)).size(12.0).color(MUTED));});
                    // Machinery limits are drawn only when their unit, if given, matches the archive unit.
                    let limit=self.limits.get(&series.tag.name);
                    let mismatch=limit.filter(|l|!l.matches_unit(&series.tag.unit));
                    let limit=limit.filter(|_|mismatch.is_none());
                    if let Some(last)=&series.last {
                        let status=if last.good(){tr!("Last: {:.4} {}","Ultima: {:.4} {}",last.value,series.tag.unit)}else{t("Last reading invalid / overridden","Ultima lettura non valida / forzata").into()};
                        ui.horizontal_wrapped(|ui| {
                            ui.label(status);ui.colored_label(MUTED,last.at.format(FORMAT).to_string());
                            if series.valid_count>0 {ui.small(tr!("Min {:.4}   Max {:.4}   |   {} valid / {} readings","Min {:.4}   Max {:.4}   |   {} valide / {} letture",series.minimum,series.maximum,series.valid_count,series.count));}
                            if let Some(l)=limit {let v=|v:Option<f64>|v.map_or("-".to_string(),|v|v.to_string());ui.small(tr!("|   Limits: min {}  max {}","|   Limiti: min {}  max {}",v(l.min),v(l.max)));}
                        });
                    } else {ui.colored_label(MUTED,t("No readings in the period. No data does not mean zero.","Nessuna lettura nel periodo. Nessun dato non significa zero."));}
                    if let Some(prior)=&series.previous {ui.small(tr!("Previous record: {} (does not confirm the state in the period)","Record precedente: {} (non conferma lo stato nel periodo)",prior.stamp));}
                    if let Some(l)=mismatch {ui.colored_label(Color32::from_rgb(155,94,20),tr!("Machinery limits ignored: unit {} differs from the archive.","Limiti dei macchinari ignorati: unita {} diversa dall'archivio.",l.unit));}
                    if limit.is_some_and(|l|l.unit.is_empty()) && !series.tag.unit.is_empty() {ui.colored_label(Color32::from_rgb(155,94,20),t("Limits have no unit: verify against the archive before use.","Limiti senza unita: verificarli rispetto all'archivio prima dell'uso."));}
                    let start=trends.request.start.and_utc().timestamp() as f64;
                    let end=trends.request.end.and_utc().timestamp() as f64;
                    let (low,high)=if series.minimum.is_finite() && series.maximum.is_finite(){
                        // Limits close to the readings join the view; far ones appear on zoom out.
                        let reach=((series.maximum-series.minimum)*0.25).max(0.5);
                        let near=|v:&f64|*v>=series.minimum-reach && *v<=series.maximum+reach;
                        let (a,b)=limit.map_or((series.minimum,series.maximum),|l|(l.min.filter(near).map_or(series.minimum,|v|v.min(series.minimum)),l.max.filter(near).map_or(series.maximum,|v|v.max(series.maximum))));
                        let pad=((b-a)*0.1).max(0.5);(a-pad,b+pad)}else{(-1.0,1.0)};
                    let plot=Plot::new(("trend",index,self.manager.generation)).height(plot_height);
                    let plot=if reset {plot.reset()}else{plot};
                    let response=plot
                        .include_x(start).include_x(end).include_y(low).include_y(high)
                        .x_grid_spacer(date_grid)
                        .x_axis_formatter(|mark,range|time_label(mark.value,if range.end()-range.start()>31536000.0 {"%m/%Y"}else if range.end()-range.start()>172800.0 {"%d/%m"}else{"%d/%m %H:%M"}))
                        .label_formatter(|_,point|format!("{}\n{:.5}",time_label(point.x,"%d/%m/%Y %H:%M:%S"),point.y))
                        .show(ui,|plot| {
                            let mut cursor=start;
                            for (a,b) in trends.coverage.iter().map(|(a,b)|(a.and_utc().timestamp() as f64,b.and_utc().timestamp() as f64)).chain(std::iter::once((end,end))) {
                                if a>cursor {plot.polygon(Polygon::new(t("Missing archive","Archivio mancante"),vec![[cursor,low],[a,low],[a,high],[cursor,high]]).fill_color(Color32::from_gray(225)).stroke(Stroke::NONE).allow_hover(false));}
                                cursor=cursor.max(b);
                            }
                            let mut flags:[Vec<[f64;2]>;3]=Default::default();
                            if trends.raw {
                                // Rebuilt from the raw readings only when the shown range or width changes.
                                let b=plot.plot_bounds();
                                let key=(b.min()[0],b.max()[0],plot.transform().frame().width().max(1.0) as usize);
                                let mut view=cache.view.borrow_mut();
                                if view.as_ref().is_none_or(|v|v.key!=key) {*view=Some(RawView::new(&series.raw,key,series.tag.kind=="Digital"));}
                                if let Some(v)=view.as_ref() {
                                    for line in &v.segments {plot.line(Line::new("Trend",line.clone()).color(COLORS[index]).width(1.6_f32));}
                                    plot.points(Points::new(t("Raw readings","Letture raw"),v.points.iter().map(|&i|[series.raw[i].x,series.raw[i].value]).collect::<Vec<_>>()).radius(2.2_f32).color(COLORS[index]));
                                    for (j,xs) in v.flags.iter().enumerate() {flags[j]=xs.iter().map(|&x|[x,low+(high-low)*j as f64*0.04]).collect();}
                                }
                            } else {
                                for line in &cache.segments {plot.line(Line::new("Trend",line.clone()).color(COLORS[index]).width(1.6_f32));}
                                plot.points(Points::new(t("Retained readings","Letture conservate"),cache.points.iter().map(|p|[p.x(),p.value]).collect::<Vec<_>>()).radius(2.2_f32).color(COLORS[index]));
                                for (i,bucket) in &series.buckets {
                                    let x=start+(*i as f64+0.5)*(end-start)/trends.request.buckets as f64;
                                    for (j,present) in [bucket.bad>0,bucket.alarm>0,bucket.inhibit>0].into_iter().enumerate() {
                                        if present {flags[j].push([x,low+(high-low)*j as f64*0.04]);}
                                    }
                                }
                            }
                            for (points,color,label) in [(std::mem::take(&mut flags[0]),Color32::from_rgb(220,139,29),t("Invalid / overridden","Non valide / forzate")),(std::mem::take(&mut flags[1]),Color32::from_rgb(192,53,60),t("Alarm recorded","Allarme registrato")),(std::mem::take(&mut flags[2]),Color32::from_rgb(139,83,171),t("Inhibit recorded","Inibizione registrata"))] {plot.points(Points::new(label,points).color(color).radius(3.0_f32));}
                            plot.pointer_coordinate()
                        });
                    // Bounds seen inside the plot lag one frame: redraw until the raw view matches them.
                    if trends.raw {
                        let b=response.transform.bounds();
                        let key=(b.min()[0],b.max()[0],response.transform.frame().width().max(1.0) as usize);
                        if cache.view.borrow().as_ref().is_none_or(|v|v.key!=key) {ui.ctx().request_repaint();}
                        if let Some(v)=cache.view.borrow().as_ref() {
                            let s=&v.summary;
                            ui.horizontal_wrapped(|ui| {
                                ui.small(tr!("Shown: {} readings · {} valid","Visibile: {} letture · {} valide",s.count,s.valid));
                                if s.valid>0 {ui.small(tr!("·  min {:.4}  max {:.4}  mean {:.4}  σ {:.4} {}","·  min {:.4}  max {:.4}  media {:.4}  σ {:.4} {}",s.min,s.max,s.mean,s.std,series.tag.unit));}
                                if let Some((rises,falls))=s.changes {ui.small(tr!("·  state changes {} (↑ {} · ↓ {}) between consecutive valid readings","·  cambi di stato {} (↑ {} · ↓ {}) tra letture valide consecutive",rises+falls,rises,falls));}
                            });
                        }
                    }
                    let painter=ui.painter().with_clip_rect(*response.transform.frame());
                    for bar in &cache.bars {
                        painter.line_segment([response.transform.position_from_point(&bar[0].into()),response.transform.position_from_point(&bar[1].into())],Stroke::new(1.0_f32,COLORS[index].gamma_multiply(0.35)));
                    }
                    // Faint red wash beyond each limit plus a dashed edge; follows zoom and pan.
                    if let Some(l)=limit {
                        let frame=*response.transform.frame();
                        for (value,above) in [(l.max,true),(l.min,false)] {
                            let Some(value)=value else {continue};
                            let y=response.transform.position_from_point(&egui_plot::PlotPoint::new(start,value)).y;
                            let edge=y.clamp(frame.top(),frame.bottom());
                            let zone=if above {frame.top()..=edge} else {edge..=frame.bottom()};
                            painter.rect_filled(egui::Rect::from_x_y_ranges(frame.x_range(),zone),0.0,Color32::from_rgba_unmultiplied(192,53,60,20));
                            if edge==y {painter.extend(egui::Shape::dashed_line(&[egui::pos2(frame.left(),y),egui::pos2(frame.right(),y)],Stroke::new(1.0_f32,Color32::from_rgba_unmultiplied(192,53,60,150)),6.0,4.0));}
                        }
                    }
                    if trends.raw && response.response.hovered()
                        && let Some(pointer)=response.inner {
                            let view=cache.view.borrow();
                            let near=view.as_ref().and_then(|v|v.points.iter().map(|&i|&series.raw[i]).min_by(|a,b|(a.x-pointer.x).abs().total_cmp(&(b.x-pointer.x).abs())));
                            if let Some(r)=near {
                                response.response.on_hover_ui(|ui| {
                                    let at=DateTime::from_timestamp_micros((r.x*1e6).round() as i64).map(|t|t.format("%Y-%m-%d %H:%M:%S%.f").to_string()).unwrap_or_default();
                                    ui.label(series.tag.description.as_str());ui.label(format!("{} · {:.6} {}",at,r.value,series.tag.unit));
                                    ui.small(tr!("{} · row {}","{} · riga {}",trends.sources.get(r.source as usize).and_then(|p|p.file_name()).unwrap_or_default().to_string_lossy(),r.row_id));
                                    ui.small(tr!("Alarm: {} · Inhibit: {}","Allarme: {} · Inibizione: {}",r.flags&2!=0,r.flags&4!=0));
                                });
                            }
                        }
                    else if response.response.hovered()
                        && let Some(pointer)=response.inner {
                            let near=cache.points.iter().min_by(|a,b|(a.x()-pointer.x).abs().total_cmp(&(b.x()-pointer.x).abs()));
                            if let Some(p)=near {
                                response.response.on_hover_ui(|ui| {
                                    ui.label(p.description.as_str());ui.label(format!("{} · {:.6} {}",p.stamp,p.value,series.tag.unit));
                                    ui.small(tr!("{} · row {}","{} · riga {}",p.source.file_name().unwrap_or_default().to_string_lossy(),p.row_id));
                                    if let Some(marker)=&p.marker {ui.small(format!("Min {} · Max {} · IntervalMarker {}",p.low,p.high,marker));}
                                    ui.small(tr!("Alarm: {} · Inhibit: {}","Allarme: {} · Inibizione: {}",p.flags[2],p.flags[3]));
                                });
                            }
                        }
                });ui.add_space(8.0);
            }
            ui.label(RichText::new(t("Orange: invalid/overridden data · Red: alarm recorded · Purple: inhibit · Grey: missing archives","Arancio: dati non validi/forzati · Rosso: allarme registrato · Viola: inibizione · Grigio: archivi mancanti")).size(11.0).color(MUTED));
            ui.label(RichText::new(if self.limits.is_empty() {t("Markers do not show the exact duration of states. No machinery operating limits imported (Machinery limits...).","I marcatori non indicano la durata esatta degli stati. Nessun limite dei macchinari importato (Limiti macchinari...).")} else {t("Markers do not show the exact duration of states. Red shading beyond the dashed lines: outside the machinery operating limits.","I marcatori non indicano la durata esatta degli stati. Ombreggiatura rossa oltre le linee tratteggiate: fuori dai limiti dei macchinari.")}).size(11.0).color(MUTED));
        });
        self.reset = false;
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.top(ctx);
        self.sidebar(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            self.controls(ui);
            ui.add_space(8.0);
            self.charts(ui);
        });
        egui::Window::new(t("Archive notes", "Note archivio"))
            .open(&mut self.notes)
            .default_width(650.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(c) = &self.catalog {
                        for note in &c.warnings {
                            ui.label(note);
                            ui.separator();
                        }
                    }
                });
            });
        egui::Window::new(t("Operator guide", "Guida operatore"))
            .open(&mut self.help)
            .default_size([760.0, 550.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if !crate::italian() {
                        ui.strong("The operator guide is available in Italian only.");
                        ui.separator();
                    }
                    ui.label(include_str!("../README_OPERATORE.txt"));
                });
            });
        for event in ctx.input(|i| i.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event
                && let Some(path) = self.screenshot.take()
            {
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                match save_screenshot(&path, &bytes, image.size[0] as u32, image.size[1] as u32) {
                    Ok(()) => self.status = t("Screenshot saved", "Schermata salvata").into(),
                    Err(e) => {
                        self.error = Some(tr!("Save failed: {}", "Salvataggio non riuscito: {}", e))
                    }
                }
            }
        }
        if self.screenshot.is_some()
            && !self.screenshot_requested
            && !self.busy
            && self.trends.is_some()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.screenshot_requested = true;
        }
        if self.options.smoke {
            self.frames += 1;
            let ready = self.options.folder.is_none() || self.smoke_stage == 2;
            let outcome = if let Some(e) = &self.error {
                Some(Err(e.clone()))
            } else if self.began.elapsed() > Duration::from_secs(150) {
                Some(Err("GUI smoke test timeout.".into()))
            } else if ready && self.frames > 4 && self.screenshot.is_none() {
                Some(Ok(()))
            } else {
                None
            };
            if let Some(result) = outcome {
                *self.smoke_result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.busy || self.options.smoke || self.screenshot.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}
pub fn run(options: Options) -> Result<(), String> {
    let smoke = options.smoke;
    let result = Arc::new(Mutex::new(None));
    let returned = result.clone();
    let icon = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (width, height) = icon.dimensions();
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 920.0])
            .with_min_inner_size([1000.0, 700.0])
            .with_visible(!smoke)
            .with_icon(Arc::new(egui::IconData {
                rgba: icon.into_raw(),
                width,
                height,
            })),
        ..Default::default()
    };
    eframe::run_native(
        "Vessel Trend Manager",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, options, result)))),
    )
    .map_err(|e| e.to_string())?;
    if smoke {
        returned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_else(|| Err("GUI smoke test not completed.".into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(kind: &str, seconds: i64, count: usize, bad: usize) -> archive::Series {
        let first = Sample {
            at: crate::date("2025-01-01 00:00").unwrap(),
            stamp: "2025-01-01 00:00:00".into(),
            value: 0.0,
            low: 0.0,
            high: 0.0,
            flags: [false; 4],
            source: Arc::new(PathBuf::from("Day_2025-01-01.s3db")),
            row_id: 1,
            marker: None,
            description: Arc::new("Synthetic signal".into()),
        };
        let mut last = first.clone();
        last.at += Span::seconds(seconds);
        last.row_id = 2;
        last.value = 1.0;
        last.low = 1.0;
        last.high = 1.0;
        archive::Series {
            tag: archive::Tag {
                name: "TEST".into(),
                description: "Synthetic signal".into(),
                unit: String::new(),
                kind: kind.into(),
                group: "TEST".into(),
            },
            buckets: BTreeMap::from([(
                0,
                archive::Bucket {
                    first: Some(first.clone()),
                    last: Some(last.clone()),
                    min: Some(first),
                    max: Some(last.clone()),
                    count,
                    bad,
                    alarm: 0,
                    inhibit: 0,
                },
            )]),
            raw: Vec::new(),
            count,
            valid_count: count - bad,
            last: Some(last),
            previous: None,
            minimum: 0.0,
            maximum: 1.0,
            latest_bad: None,
        }
    }
    #[test]
    fn limits_file_is_parsed_strictly_and_demo_limits_are_valid() {
        let limits = parse_limits(
            "\u{feff}# comment\ntag;min;max;unit;notes\n;1;2;bar;unmapped row\nA;1,5;/;bar\nB;;80\n",
        )
        .unwrap();
        assert_eq!(limits.len(), 2);
        assert_eq!((limits["A"].min, limits["A"].max), (Some(1.5), None));
        assert_eq!(
            (limits["B"].max, limits["B"].unit.as_str()),
            (Some(80.0), "")
        );
        for bad in ["A;3;2", "A;x;2", "A;1;2\nA;1;3", "A;NaN;2"] {
            assert!(parse_limits(bad).is_err(), "{bad}");
        }
        // Plain CSV saved by Excel uses the Windows code page, not UTF-8.
        let path = std::env::temp_dir().join(format!("vtm_limits_{}.csv", std::process::id()));
        fs::write(&path, b"tag;min;max;unit\r\nA;;80;\xB0C\r\n").unwrap();
        let excel = read_limits(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(excel.unwrap()["A"].unit, "°C");
        let demo = parse_limits(crate::demo::LIMITS_CSV).unwrap();
        assert!(crate::demo::FEATURED.iter().all(|t| demo.contains_key(*t)));
    }
    #[test]
    fn safety_limits_preserve_unit_case_and_clear_stale_values() {
        let limits = parse_limits("POWER;;5;MW").unwrap();
        assert!(limits["POWER"].matches_unit("MW"));
        assert!(!limits["POWER"].matches_unit("mW"));
        let options = Options {
            smoke: true,
            ..Default::default()
        };
        let mut app = App::new(
            &egui::Context::default(),
            options,
            Arc::new(Mutex::new(None)),
        );
        app.limits = limits;
        // The synthetic test folder deliberately has no limits file.
        let path = std::env::temp_dir().join(format!(
            "vtm_missing_limits_{}_{}.csv",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!app.read_limits_file(path));
        assert!(app.limits.is_empty());
        assert!(app.error.is_some());
    }
    #[test]
    fn screenshots_never_overwrite_files_and_small_reads_are_bounded() {
        let folder = std::env::temp_dir().join(format!(
            "vtm_security_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&folder).unwrap();
        let path = folder.join("synthetic.s3db");
        fs::write(&path, b"synthetic sentinel").unwrap();
        assert!(save_screenshot(&path, &[0; 4], 1, 1).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"synthetic sentinel");
        let image = folder.join("image.png");
        save_screenshot(&image, &[0; 4], 1, 1).unwrap();
        let original = fs::read(&image).unwrap();
        assert!(save_screenshot(&image, &[255; 4], 1, 1).is_err());
        assert_eq!(fs::read(&image).unwrap(), original);
        let large = folder.join("large.txt");
        fs::File::create(&large)
            .unwrap()
            .set_len(1_048_577)
            .unwrap();
        assert!(read_small_file(&large).is_err());
        let preferences = folder.join("preferenze.json");
        let old_temp = folder.join("preferenze.json.tmp");
        fs::write(&old_temp, b"preserve sentinel").unwrap();
        write_settings(&preferences, b"{}").unwrap();
        write_settings(&preferences, b"{\"language\":\"en\"}").unwrap();
        assert_eq!(fs::read(&preferences).unwrap(), b"{\"language\":\"en\"}");
        assert_eq!(fs::read(&old_temp).unwrap(), b"preserve sentinel");
        fs::remove_dir_all(folder).unwrap();
    }
    fn raw(values: &[f64]) -> Vec<Raw> {
        values
            .iter()
            .enumerate()
            .map(|(i, &value)| Raw {
                x: 1_700_000_000.0 + i as f64,
                value,
                row_id: i as i64 + 1,
                source: 0,
                nanos: 0,
                flags: u8::from(!value.is_finite()),
            })
            .collect()
    }
    #[test]
    fn raw_view_draws_every_reading_that_fits_and_breaks_on_invalid() {
        let all = raw(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let view = RawView::new(&all, (all[0].x, all[4].x, 100), false);
        assert_eq!(view.points, vec![0, 1, 2, 3, 4]);
        assert_eq!(view.segments.len(), 1);
        let gap = raw(&[1.0, 2.0, f64::NAN, 4.0, 5.0]);
        let view = RawView::new(&gap, (gap[0].x, gap[4].x, 100), false);
        assert_eq!(view.points, vec![0, 1, 3, 4]);
        assert_eq!(view.segments.len(), 2);
        assert_eq!(view.flags[0], vec![gap[2].x]);
        // Empty or degenerate bounds draw nothing instead of panicking.
        assert!(RawView::new(&[], (0.0, 1.0, 100), false).points.is_empty());
        assert!(
            RawView::new(&all, (f64::NAN, 1.0, 100), false)
                .points
                .is_empty()
        );
    }
    #[test]
    fn raw_view_reduction_keeps_real_extremes_and_zoom_shows_all() {
        let values: Vec<f64> = (0..100_000).map(|i| (i as f64 * 0.01).sin()).collect();
        let mut all = raw(&values);
        all[54_321].value = 9.0;
        all[12_345].value = -9.0;
        let key = (all[0].x, all[99_999].x, 200);
        let view = RawView::new(&all, key, false);
        assert!(view.points.len() <= 4 * 2 * 200);
        assert!(view.points.contains(&54_321) && view.points.contains(&12_345));
        assert!(view.points.windows(2).all(|w| w[0] < w[1]));
        // Zoomed to 100 readings on a 200 px plot: every raw reading in range is drawn.
        let view = RawView::new(&all, (all[50_000].x, all[50_099].x, 200), false);
        assert!((50_000..50_100).all(|i| view.points.contains(&i)));
    }
    #[test]
    fn shown_range_statistics_use_valid_readings_only() {
        let all = raw(&[2.0, 4.0, f64::NAN, 4.0, 5.0, 100.0]);
        let s = Summary::new(&all, all[0].x, all[4].x, false);
        assert_eq!((s.count, s.valid), (5, 4));
        assert_eq!((s.min, s.max, s.mean), (2.0, 5.0, 3.75));
        assert!((s.std - 1.0897247358851685).abs() < 1e-12);
        assert!(s.changes.is_none());
        let none = Summary::new(&all, all[5].x + 1.0, all[5].x + 2.0, false);
        assert_eq!((none.count, none.valid, none.std), (0, 0, 0.0));
    }
    #[test]
    fn digital_state_changes_are_counted_between_valid_readings_only() {
        // 0→1 rise, 1→0 fall, then a change across an invalid reading is not counted.
        let states = raw(&[0.0, 1.0, 1.0, 0.0, f64::NAN, 1.0, 1.0]);
        let s = Summary::new(&states, states[0].x, states[6].x, true);
        assert_eq!(s.changes, Some((1, 1)));
        let s = Summary::new(&states, states[1].x, states[3].x, true);
        assert_eq!(s.changes, Some((0, 1)));
    }
    #[test]
    fn reduced_digital_plot_does_not_invent_state_duration() {
        let plot = CachedPlot::new(&series("Digital", 10, 20, 0), Mode::Daily);
        assert_eq!(plot.points.len(), 2);
        assert!(plot.segments.is_empty());
    }
    #[test]
    fn invalid_samples_and_long_gaps_break_native_lines() {
        for source in [series("Analog", 10, 3, 1), series("Analog", 7200, 2, 0)] {
            assert!(CachedPlot::new(&source, Mode::Daily).segments.is_empty());
        }
        assert_eq!(
            CachedPlot::new(&series("Analog", 10, 2, 0), Mode::Daily)
                .segments
                .len(),
            1
        );
    }
}
