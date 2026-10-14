//! Read-only AMS archive adapter and bounded, cancellable background retrieval.
use crate::t;
use chrono::{Duration as Span, NaiveDate, NaiveDateTime, Timelike};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, config::DbConfig, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

pub type Outcome<T> = Result<T, String>;
pub const MAX_TAGS: usize = 4;
pub const MAX_ROWS: usize = 50_000_000;
/// Upper bound on archive reader threads; the actual count follows the host CPU.
const MAX_THREADS: usize = 4;
pub const MAX_BUCKETS: usize = 800;
pub const MAX_SECONDS: u64 = 120;
const MAX_ARCHIVES: usize = 20_000;
const MAX_CATALOG_TAGS: usize = 10_000;
const MAX_TEXT_BYTES: usize = 1_024;
const MAX_DYNAMIC_IDS: usize = 10_000;
const TAG_SQL: &str =
    "SELECT t.Name, COALESCE(t.Description,t.Name), COALESCE(t.EngUnit,''), tt.Name,
    COALESCE(a.AlarmGroupText,'') FROM TagConfig t
    JOIN TagType tt ON tt.Id=t.TagType LEFT JOIN AlarmGroup a ON a.AlarmGroupNumber=t.AlarmGroup";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Daily,
    Weekly,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => t("Daily", "Giornalieri"),
            Self::Weekly => t("Weekly", "Settimanali"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tag {
    pub name: String,
    pub description: String,
    pub unit: String,
    pub kind: String,
    pub group: String,
}
#[derive(Clone, Debug)]
pub struct Archive {
    pub path: PathBuf,
    pub mode: Mode,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub latest: NaiveDateTime,
    signature: (u64, SystemTime),
}
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub archives: Vec<Archive>,
    pub tags: BTreeMap<String, Tag>,
    pub warnings: Vec<String>,
}
impl Catalog {
    pub fn available(&self, mode: Mode) -> impl Iterator<Item = &Archive> {
        self.archives.iter().filter(move |a| a.mode == mode)
    }
    pub fn bounds(&self, mode: Mode) -> Outcome<(NaiveDateTime, NaiveDateTime)> {
        let start = self.available(mode).map(|a| a.start).min().ok_or(t(
            "No archive available for this source.",
            "Nessun archivio disponibile per questa sorgente.",
        ))?;
        let latest = self
            .available(mode)
            .map(|a| a.latest)
            .max()
            .ok_or(t("Empty source.", "Sorgente vuota."))?;
        let end = latest
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .and_then(|t| t.checked_add_signed(Span::minutes(1)))
            .ok_or(t("Date out of range.", "Data fuori intervallo."))?;
        Ok((start, end))
    }
}

pub fn timestamp(text: &str) -> Outcome<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f").map_err(|_| {
        t(
            "Invalid archive timestamp.",
            "Timestamp archivio non valido.",
        )
        .into()
    })
}
fn sql_time(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%d %H:%M:%S%.f").to_string()
}
fn fingerprint(path: &Path) -> Outcome<(u64, SystemTime)> {
    let m = fs::metadata(path).map_err(|e| e.to_string())?;
    Ok((m.len(), m.modified().map_err(|e| e.to_string())?))
}
fn check(cancel: &AtomicBool, deadline: Instant) -> Outcome<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(t("Loading cancelled.", "Caricamento annullato.").into());
    }
    if Instant::now() >= deadline {
        return Err(t(
            "Time limit reached: shorten the period or reduce the signals.",
            "Tempo massimo raggiunto: ridurre periodo o parametri.",
        )
        .into());
    }
    Ok(())
}
/// SQLite URI for an immutable read: no lock, -shm or -wal file is created next to the archive.
fn uri(path: &Path) -> Outcome<String> {
    let mut text = path
        .to_str()
        .ok_or(t(
            "Unsupported characters in the archive path.",
            "Caratteri non supportati nel percorso archivio.",
        ))?
        .replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/UNC/") {
        text = format!("//{rest}");
    } else if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_string();
    }
    let mut uri = String::from(if text.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    Ok(uri)
}
fn connect(path: &Path, cancel: Arc<AtomicBool>, deadline: Instant) -> Outcome<Connection> {
    check(&cancel, deadline)?;
    // Immutable reads skip a pending WAL or hot journal: refuse such archives instead.
    for suffix in ["-wal", "-journal"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let pending = match fs::metadata(&side) {
            Ok(meta) => meta.len() > 0,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.to_string()),
        };
        if pending {
            return Err(t(
                "Archive with a pending journal or WAL file: use a closed export.",
                "Archivio con file journal o WAL in sospeso: usare un export chiuso.",
            )
            .into());
        }
    }
    let con = Connection::open_with_flags(
        uri(path)?,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    con.busy_timeout(Duration::from_secs(1))
        .map_err(|e| e.to_string())?;
    // Untrusted archives: no schema-defined functions/views side effects, no corruption-prone writes.
    con.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(|e| e.to_string())?;
    // SQLite's heap cap is process-wide, shared by all readers. Temporary sorts stay off disk.
    con.execute_batch("PRAGMA hard_heap_limit=67108864; PRAGMA temp_store=MEMORY; PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA cell_size_check=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-4096;")
        .map_err(|e| e.to_string())?;
    con.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)
        .map_err(|e| e.to_string())?;
    con.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
        .map_err(|e| e.to_string())?;
    con.progress_handler(
        2000,
        Some(move || cancel.load(Ordering::Relaxed) || Instant::now() >= deadline),
    )
    .map_err(|e| e.to_string())?;
    // Expected input is ordinary stored tables, never schema-defined queries or virtual tables.
    for name in [
        "TagConfig",
        "TagType",
        "AlarmGroup",
        "TagConfigDynamic",
        "TagValue",
    ] {
        let kind: Option<String> = con
            .query_row(
                "SELECT type FROM pragma_table_list WHERE schema='main' AND name=?",
                [name],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if kind.as_deref() != Some("table") {
            return Err(t(
                "Expected ordinary archive tables.",
                "Richieste tabelle archivio ordinarie.",
            )
            .into());
        }
    }
    Ok(con)
}
fn read_tag(row: &Row<'_>) -> rusqlite::Result<Tag> {
    Ok(Tag {
        name: read_text(row, 0)?,
        description: read_text(row, 1)?,
        unit: read_text(row, 2)?,
        kind: read_text(row, 3)?,
        group: Some(read_text(row, 4)?)
            .filter(|g| !g.is_empty())
            .unwrap_or_else(|| t("Other signals", "Altri segnali").into()),
    })
}
fn read_text(row: &Row<'_>, column: usize) -> rusqlite::Result<String> {
    if let rusqlite::types::ValueRef::Text(value) = row.get_ref(column)?
        && value.len() > MAX_TEXT_BYTES
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    row.get(column)
}
fn nominal(path: &Path, mode: Mode) -> Outcome<(NaiveDateTime, NaiveDateTime)> {
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parse = |s: &str| {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
    };
    let pair = match mode {
        Mode::Daily => name
            .strip_prefix("Day_")
            .and_then(|s| parse(s).map(|d| (d, d))),
        Mode::Weekly => name
            .strip_prefix("Week_from_")
            .and_then(|s| s.split_once("_to_"))
            .and_then(|(a, b)| Some((parse(a)?, parse(b)?))),
    };
    let (start, last) = pair.ok_or(t(
        "Expected name: Day_YYYY-MM-DD or Week_from_YYYY-MM-DD_to_YYYY-MM-DD.",
        "Nome richiesto: Day_AAAA-MM-GG o Week_from_AAAA-MM-GG_to_AAAA-MM-GG.",
    ))?;
    let end = last
        .checked_add_signed(Span::days(1))
        .ok_or(t("Date out of range.", "Data fuori intervallo."))?;
    if end <= start {
        return Err(t("Inconsistent nominal dates.", "Date nominali incoerenti.").into());
    }
    Ok((start, end))
}

pub fn scan(folder: &Path, cancel: Arc<AtomicBool>, progress: &dyn Fn(String)) -> Outcome<Catalog> {
    if !folder.is_dir() {
        return Err(t(
            "Choose an existing data folder.",
            "Scegliere una cartella dati esistente.",
        )
        .into());
    }
    let deadline = Instant::now() + Duration::from_secs(MAX_SECONDS);
    let mut paths = BTreeSet::new();
    for base in [
        folder.to_path_buf(),
        folder.join("days"),
        folder.join("weeks"),
    ] {
        if !base.is_dir() {
            continue;
        }
        for item in fs::read_dir(base).map_err(|e| e.to_string())? {
            check(&cancel, deadline)?;
            let path = item.map_err(|e| e.to_string())?.path();
            if path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("s3db"))
                && path.is_file()
            {
                paths.insert(path.canonicalize().map_err(|e| e.to_string())?);
                if paths.len() > MAX_ARCHIVES {
                    return Err(t("Too many archive files.", "Troppi file archivio.").into());
                }
            }
        }
    }
    if paths.is_empty() {
        return Err(t(
            "No .s3db files: choose EXPORT, days or weeks.",
            "Nessun .s3db: scegliere EXPORT oppure days o weeks.",
        )
        .into());
    }
    let mut catalog = Catalog::default();
    let mut ranks = BTreeMap::new();
    let mut changed = BTreeSet::new();
    for (index, path) in paths.iter().enumerate() {
        check(&cancel, deadline)?;
        progress(tr!(
            "Catalog: {}/{} archives",
            "Catalogo: {}/{} archivi",
            index + 1,
            paths.len()
        ));
        let inspect = || -> Outcome<(Archive, Vec<Tag>)> {
            let signature = fingerprint(path)?;
            let con = connect(path, cancel.clone(), deadline)?;
            let mut stmt = con
                .prepare("PRAGMA table_info(TagValue)")
                .map_err(|e| e.to_string())?;
            let columns: BTreeSet<String> = stmt
                .query_map([], |r| r.get(1))
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| e.to_string())?;
            if ![
                "Id",
                "Value",
                "Timestamp",
                "TagConfigDynamic",
                "IsInvalid",
                "IsOverride",
                "IsInAlarm",
                "IsInhibit",
            ]
            .iter()
            .all(|c| columns.contains(*c))
            {
                return Err(t(
                    "Unrecognized archive schema.",
                    "Schema archivio non riconosciuto.",
                )
                .into());
            }
            let mode = if ["Min", "Max", "IntervalMarker"]
                .iter()
                .all(|c| columns.contains(*c))
            {
                Mode::Weekly
            } else {
                Mode::Daily
            };
            let (start, end) = nominal(path, mode)?;
            if catalog
                .available(mode)
                .any(|a| start < a.end && end > a.start)
            {
                return Err(t(
                    "Period overlaps an archive already included.",
                    "Periodo sovrapposto a un archivio gia incluso.",
                )
                .into());
            }
            let last: Option<String> = con.query_row("SELECT Timestamp FROM TagValue WHERE Timestamp>=? AND Timestamp<? ORDER BY Timestamp DESC LIMIT 1",params![sql_time(start),sql_time(end)],|r| read_text(r, 0)).optional().map_err(|e| e.to_string())?;
            let latest = timestamp(&last.ok_or(t(
                "No readings in the nominal period.",
                "Nessuna lettura nel periodo nominale.",
            ))?)?;
            let mut stmt = con.prepare(TAG_SQL).map_err(|e| e.to_string())?;
            let tags = stmt
                .query_map([], read_tag)
                .map_err(|e| e.to_string())?
                .take(MAX_CATALOG_TAGS + 1)
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            if tags.len() > MAX_CATALOG_TAGS {
                return Err(t("Too many catalog entries.", "Troppe voci nel catalogo.").into());
            }
            if fingerprint(path)? != signature {
                return Err(t(
                    "File changed during the scan.",
                    "File cambiato durante la scansione.",
                )
                .into());
            }
            Ok((
                Archive {
                    path: path.clone(),
                    mode,
                    start,
                    end,
                    latest,
                    signature,
                },
                tags,
            ))
        };
        match inspect() {
            Ok((archive, tags)) => {
                for tag in tags {
                    if let Some(old) = catalog.tags.get(&tag.name)
                        && (
                            old.description.as_str(),
                            old.unit.as_str(),
                            old.kind.as_str(),
                        ) != (
                            tag.description.as_str(),
                            tag.unit.as_str(),
                            tag.kind.as_str(),
                        )
                    {
                        changed.insert(tag.name.clone());
                    }
                    if ranks.get(&tag.name).is_none_or(|t| archive.latest >= *t) {
                        ranks.insert(tag.name.clone(), archive.latest);
                        catalog.tags.insert(tag.name.clone(), tag);
                        if catalog.tags.len() > MAX_CATALOG_TAGS {
                            return Err(t(
                                "Too many catalog entries.",
                                "Troppe voci nel catalogo.",
                            )
                            .into());
                        }
                    }
                }
                catalog.archives.push(archive);
            }
            Err(e) => {
                check(&cancel, deadline)?;
                catalog.warnings.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    if catalog.archives.is_empty() {
        return Err(tr!(
            "No usable archive. {}",
            "Nessun archivio utilizzabile. {}",
            catalog
                .warnings
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if !changed.is_empty() {
        catalog.warnings.push(tr!(
            "Metadata changed over time: {}. The catalog shows the most recent.",
            "Metadati variati nello storico: {}. Il catalogo mostra i piu recenti.",
            changed.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    catalog.archives.sort_by_key(|a| a.start);
    check(&cancel, deadline)?;
    Ok(catalog)
}

#[derive(Clone, Debug)]
pub struct Request {
    pub tags: Vec<String>,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub mode: Mode,
    pub buckets: usize,
}
impl Request {
    pub fn validate(&self) -> Outcome<()> {
        if self.tags.is_empty()
            || self.tags.len() > MAX_TAGS
            || self.tags.iter().collect::<BTreeSet<_>>().len() != self.tags.len()
        {
            return Err(t(
                "Choose one to four different signals.",
                "Scegliere da uno a quattro parametri diversi.",
            )
            .into());
        }
        if self.end <= self.start {
            return Err(t(
                "The end must be after the start.",
                "La fine deve essere successiva all'inizio.",
            )
            .into());
        }
        if !(1..=MAX_BUCKETS).contains(&self.buckets) {
            return Err(t("Invalid resolution.", "Risoluzione non valida.").into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct Sample {
    pub at: NaiveDateTime,
    pub stamp: String,
    pub value: f64,
    pub low: f64,
    pub high: f64,
    pub flags: [bool; 4],
    pub source: Arc<PathBuf>,
    pub row_id: i64,
    pub marker: Option<String>,
    pub description: Arc<String>,
}
impl Sample {
    pub fn good(&self) -> bool {
        !self.flags[0]
            && !self.flags[1]
            && self.value.is_finite()
            && self.low.is_finite()
            && self.high.is_finite()
            && self.low <= self.high
    }
    fn key(&self) -> (NaiveDateTime, i64) {
        (self.at, self.row_id)
    }
    pub fn x(&self) -> f64 {
        self.at.and_utc().timestamp_micros() as f64 / 1_000_000.0
    }
}
fn sample(
    row: &Row<'_>,
    path: Arc<PathBuf>,
    description: Arc<String>,
    mode: Mode,
) -> Outcome<Sample> {
    let get = || -> rusqlite::Result<_> {
        let value = row.get::<_, Option<f64>>(2)?.unwrap_or(f64::NAN);
        let weekly = mode == Mode::Weekly;
        let low = if weekly {
            row.get::<_, Option<f64>>(7)?.unwrap_or(value)
        } else {
            value
        };
        let high = if weekly {
            row.get::<_, Option<f64>>(8)?.unwrap_or(value)
        } else {
            value
        };
        let marker = if weekly {
            Some(match row.get_ref(9)? {
                rusqlite::types::ValueRef::Null => "NULL".into(),
                rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                rusqlite::types::ValueRef::Real(v) => v.to_string(),
                rusqlite::types::ValueRef::Text(_) => read_text(row, 9)?,
                _ => "binario".into(),
            })
        } else {
            None
        };
        Ok((
            row.get::<_, i64>(0)?,
            read_text(row, 1)?,
            value,
            low,
            high,
            [
                row.get::<_, i64>(3)? != 0,
                row.get::<_, i64>(4)? != 0,
                row.get::<_, i64>(5)? != 0,
                row.get::<_, i64>(6)? != 0,
            ],
            marker,
        ))
    };
    let (row_id, stamp, value, low, high, flags, marker) = get().map_err(|e| e.to_string())?;
    Ok(Sample {
        at: timestamp(&stamp)?,
        stamp,
        value,
        low,
        high,
        flags,
        source: path,
        row_id,
        marker,
        description,
    })
}

#[derive(Clone, Debug, Default)]
pub struct Bucket {
    pub first: Option<Sample>,
    pub last: Option<Sample>,
    pub min: Option<Sample>,
    pub max: Option<Sample>,
    pub bad: usize,
    pub alarm: usize,
    pub inhibit: usize,
    pub count: usize,
}
impl Bucket {
    fn add(&mut self, p: &Sample) {
        self.count += 1;
        self.alarm += usize::from(p.flags[2]);
        self.inhibit += usize::from(p.flags[3]);
        if !p.good() {
            self.bad += 1;
            return;
        }
        if self.first.as_ref().is_none_or(|v| p.key() < v.key()) {
            self.first = Some(p.clone());
        }
        if self.last.as_ref().is_none_or(|v| p.key() > v.key()) {
            self.last = Some(p.clone());
        }
        if self.min.as_ref().is_none_or(|v| p.low < v.low) {
            self.min = Some(p.clone());
        }
        if self.max.as_ref().is_none_or(|v| p.high > v.high) {
            self.max = Some(p.clone());
        }
    }
    /// Folds a later archive's bucket in; same tie rules as `add`, so results match a serial read.
    fn merge(&mut self, other: Bucket) {
        self.count += other.count;
        self.bad += other.bad;
        self.alarm += other.alarm;
        self.inhibit += other.inhibit;
        if let Some(p) = other.first
            && self.first.as_ref().is_none_or(|v| p.key() < v.key())
        {
            self.first = Some(p);
        }
        if let Some(p) = other.last
            && self.last.as_ref().is_none_or(|v| p.key() > v.key())
        {
            self.last = Some(p);
        }
        if let Some(p) = other.min
            && self.min.as_ref().is_none_or(|v| p.low < v.low)
        {
            self.min = Some(p);
        }
        if let Some(p) = other.max
            && self.max.as_ref().is_none_or(|v| p.high > v.high)
        {
            self.max = Some(p);
        }
    }
    pub fn points(&self) -> Vec<&Sample> {
        let mut values: Vec<_> = [&self.first, &self.last, &self.min, &self.max]
            .into_iter()
            .flatten()
            .collect();
        values.sort_by_key(|p| p.key());
        values.dedup_by(|a, b| a.row_id == b.row_id && a.source == b.source);
        values
    }
}
#[derive(Debug)]
pub struct Series {
    pub tag: Tag,
    pub buckets: BTreeMap<usize, Bucket>,
    pub count: usize,
    pub valid_count: usize,
    pub last: Option<Sample>,
    pub previous: Option<Sample>,
    pub minimum: f64,
    pub maximum: f64,
}
impl Series {
    fn new(tag: Tag) -> Self {
        Self {
            tag,
            buckets: BTreeMap::new(),
            count: 0,
            valid_count: 0,
            last: None,
            previous: None,
            minimum: f64::INFINITY,
            maximum: f64::NEG_INFINITY,
        }
    }
    fn add(&mut self, p: Sample, index: usize) {
        self.count += 1;
        if self.last.as_ref().is_none_or(|v| p.key() > v.key()) {
            self.last = Some(p.clone());
        }
        if p.good() {
            self.valid_count += 1;
            self.minimum = self.minimum.min(p.low);
            self.maximum = self.maximum.max(p.high);
        }
        self.buckets.entry(index).or_default().add(&p);
    }
    fn merge(&mut self, other: Series) {
        self.count += other.count;
        self.valid_count += other.valid_count;
        self.minimum = self.minimum.min(other.minimum);
        self.maximum = self.maximum.max(other.maximum);
        if let Some(p) = other.last
            && self.last.as_ref().is_none_or(|v| p.key() > v.key())
        {
            self.last = Some(p);
        }
        if let Some(p) = other.previous
            && self.previous.as_ref().is_none_or(|v| p.key() > v.key())
        {
            self.previous = Some(p);
        }
        for (index, bucket) in other.buckets {
            match self.buckets.entry(index) {
                Entry::Vacant(e) => {
                    e.insert(bucket);
                }
                Entry::Occupied(mut e) => e.get_mut().merge(bucket),
            }
        }
    }
}
#[derive(Debug)]
pub struct Trends {
    pub request: Request,
    pub series: Vec<Series>,
    pub coverage: Vec<(NaiveDateTime, NaiveDateTime)>,
    pub seconds: f64,
}
pub fn load(
    catalog: &Catalog,
    request: Request,
    cancel: Arc<AtomicBool>,
    progress: &dyn Fn(String),
) -> Outcome<Trends> {
    request.validate()?;
    let began = Instant::now();
    let deadline = began + Duration::from_secs(MAX_SECONDS);
    let archives: Vec<_> = catalog
        .available(request.mode)
        .filter(|a| a.start < request.end && a.end > request.start)
        .collect();
    if archives.is_empty() {
        return Err(t(
            "No archive of this source in the chosen period.",
            "Nessun archivio di questa sorgente nel periodo scelto.",
        )
        .into());
    }
    let mut series: Vec<_> = request
        .tags
        .iter()
        .map(|n| {
            catalog
                .tags
                .get(n)
                .cloned()
                .map(Series::new)
                .ok_or_else(|| {
                    t(
                        "Signal not present in the catalog.",
                        "Parametro non presente nel catalogo.",
                    )
                    .into()
                })
        })
        .collect::<Outcome<_>>()?;
    let width = (request.end - request.start).num_milliseconds() as f64 / request.buckets as f64;
    let total = AtomicUsize::new(0);
    let columns = if request.mode == Mode::Weekly {
        "Id, Timestamp, Value, COALESCE(IsInvalid,0), COALESCE(IsOverride,0), COALESCE(IsInAlarm,0), COALESCE(IsInhibit,0), Min, Max, IntervalMarker"
    } else {
        "Id, Timestamp, Value, COALESCE(IsInvalid,0), COALESCE(IsOverride,0), COALESCE(IsInAlarm,0), COALESCE(IsInhibit,0)"
    };
    let templates: Vec<_> = series.iter().map(|s| s.tag.clone()).collect();
    // One archive per call, on its own connection; partial series are merged in archive order.
    let read = |archive: &Archive| -> Outcome<Vec<Series>> {
        let mut partial: Vec<_> = templates.iter().cloned().map(Series::new).collect();
        check(&cancel, deadline)?;
        if fingerprint(&archive.path)? != archive.signature {
            return Err(t(
                "Archives changed: refresh the catalog.",
                "Archivi cambiati: aggiornare il catalogo.",
            )
            .into());
        }
        let mut run = || -> Outcome<()> {
            let con = connect(&archive.path, cancel.clone(), deadline)?;
            let path = Arc::new(archive.path.clone());
            let lo = request.start.max(archive.start);
            let hi = request.end.min(archive.end);
            let sql_lo = lo
                .with_nanosecond(0)
                .ok_or(t("Invalid date.", "Data non valida."))?;
            let sql_hi = hi
                .with_nanosecond(0)
                .and_then(|v| v.checked_add_signed(Span::seconds(i64::from(hi.nanosecond() > 0))))
                .ok_or(t("Invalid date.", "Data non valida."))?;
            for target in &mut partial {
                check(&cancel, deadline)?;
                let tag = con
                    .query_row(
                        &format!("{TAG_SQL} WHERE t.Name=?"),
                        [&target.tag.name],
                        read_tag,
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                let Some(tag) = tag else {
                    continue;
                };
                if tag.unit != target.tag.unit || tag.kind != target.tag.kind {
                    return Err(tr!(
                        "{}: unit or type changed over time.",
                        "{}: unita o tipo variati nello storico.",
                        tag.name
                    ));
                }
                let description = Arc::new(tag.description);
                let mut stmt = con.prepare("SELECT d.Id FROM TagConfigDynamic d JOIN TagConfig t ON t.Id=d.TagConfig WHERE t.Name=?").map_err(|e| e.to_string())?;
                let ids: Vec<i64> = stmt
                    .query_map([&target.tag.name], |r| r.get(0))
                    .map_err(|e| e.to_string())?
                    .take(MAX_DYNAMIC_IDS + 1)
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| e.to_string())?;
                if ids.len() > MAX_DYNAMIC_IDS {
                    return Err(t(
                        "Too many signal versions.",
                        "Troppe versioni del parametro.",
                    )
                    .into());
                }
                for id in ids {
                    if request.mode == Mode::Daily {
                        let mut stmt = con.prepare(&format!("SELECT {columns} FROM TagValue WHERE TagConfigDynamic=? AND Timestamp<? ORDER BY Timestamp DESC,Id DESC LIMIT 1")).map_err(|e| e.to_string())?;
                        let mut rows = stmt
                            .query(params![id, sql_time(request.start)])
                            .map_err(|e| e.to_string())?;
                        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
                            let prior =
                                sample(row, path.clone(), description.clone(), request.mode)?;
                            if prior.at < request.start
                                && target
                                    .previous
                                    .as_ref()
                                    .is_none_or(|v| prior.key() > v.key())
                            {
                                target.previous = Some(prior);
                            }
                        }
                    }
                    // Stream indexed rows without ORDER BY or an unbounded raw-series allocation.
                    let mut stmt = con.prepare(&format!("SELECT {columns} FROM TagValue WHERE TagConfigDynamic=? AND Timestamp>=? AND Timestamp<?")).map_err(|e| e.to_string())?;
                    let mut rows = stmt
                        .query(params![id, sql_time(sql_lo), sql_time(sql_hi)])
                        .map_err(|e| e.to_string())?;
                    let mut local = 0usize;
                    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                        local += 1;
                        // Shared counter updated in batches to keep threads off one cache line.
                        if local.is_multiple_of(2048) {
                            if total.fetch_add(2048, Ordering::Relaxed) + 2048 > MAX_ROWS {
                                return Err(t("Too many samples", "Troppi campioni").into());
                            }
                            check(&cancel, deadline)?;
                        }
                        let point = sample(row, path.clone(), description.clone(), request.mode)?;
                        if point.at >= lo && point.at < hi {
                            let bucket = (((point.at - request.start).num_milliseconds() as f64
                                / width) as usize)
                                .min(request.buckets - 1);
                            target.add(point, bucket);
                        }
                    }
                    total.fetch_add(local % 2048, Ordering::Relaxed);
                }
            }
            Ok(())
        };
        if let Err(e) = run() {
            check(&cancel, deadline)?;
            return Err(format!(
                "{}: {e}",
                archive
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ));
        }
        if fingerprint(&archive.path)? != archive.signature {
            return Err(t(
                "Archive modified while reading: refresh.",
                "Archivio modificato durante la lettura: aggiornare.",
            )
            .into());
        }
        Ok(partial)
    };
    // Portable parallelism: std threads only, sized to the host CPU (1 if unknown), no CPU-specific code.
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_THREADS)
        .min(archives.len());
    progress(tr!(
        "Reading 0/{} archives",
        "Lettura 0/{} archivi",
        archives.len()
    ));
    // At most one batch of partial results is retained, regardless of history length.
    for (batch_index, batch) in archives.chunks(threads).enumerate() {
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let mut parts: Vec<Option<Outcome<Vec<Series>>>> = batch.iter().map(|_| None).collect();
        std::thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel();
            for _ in 0..threads {
                let (sender, read, next, stop, archives) =
                    (sender.clone(), &read, &next, &stop, batch);
                scope.spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(archive) = archives.get(index) else {
                            break;
                        };
                        let part = read(archive);
                        if part.is_err() {
                            stop.store(true, Ordering::Relaxed);
                        }
                        if sender.send((index, part)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);
            for (done, (index, part)) in receiver.iter().enumerate() {
                progress(tr!(
                    "Reading {}/{} archives | {} samples",
                    "Lettura {}/{} archivi | {} campioni",
                    batch_index * threads + done + 1,
                    archives.len(),
                    total.load(Ordering::Relaxed)
                ));
                parts[index] = Some(part);
            }
        });
        check(&cancel, deadline)?;
        for part in parts {
            let part = part.ok_or(t(
                "Incomplete read: retry.",
                "Lettura incompleta: riprovare.",
            ))??;
            for (target, piece) in series.iter_mut().zip(part) {
                target.merge(piece);
            }
        }
    }
    check(&cancel, deadline)?;
    let read_rows = total.load(Ordering::Relaxed);
    if read_rows > MAX_ROWS {
        return Err(tr!(
            "Too many samples in the requested period (over {} million, read {}): shorten the period, reduce the signals or choose weekly.",
            "Troppi campioni nel periodo richiesto (oltre {} milioni, letti {}): ridurre periodo o parametri, oppure scegliere i settimanali.",
            MAX_ROWS / 1_000_000,
            read_rows
        ));
    }
    let coverage = archives
        .iter()
        .map(|a| (a.start.max(request.start), a.end.min(request.end)))
        .collect();
    Ok(Trends {
        request,
        series,
        coverage,
        seconds: began.elapsed().as_secs_f64(),
    })
}

pub enum Job {
    Scan(PathBuf),
    /// Builds the synthetic demo archives in the folder if needed, then scans it.
    Demo(PathBuf),
    Load(Arc<Catalog>, Request),
}
pub enum Loaded {
    Catalog(Arc<Catalog>),
    Trends(Trends),
}
struct Pending {
    generation: u64,
    cancel: Arc<AtomicBool>,
    job: Job,
}
struct Shared {
    pending: Mutex<Option<Pending>>,
    wake: Condvar,
    closed: AtomicBool,
    generation: AtomicU64,
    output: Mutex<Option<(u64, Outcome<Loaded>)>>,
    progress: Mutex<Option<(u64, String)>>,
}
pub struct Manager {
    shared: Arc<Shared>,
    cancel: Arc<AtomicBool>,
    pub generation: u64,
}
impl Manager {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            pending: Mutex::new(None),
            wake: Condvar::new(),
            closed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            output: Mutex::new(None),
            progress: Mutex::new(None),
        });
        let worker = shared.clone();
        std::thread::spawn(move || {
            loop {
                let pending = {
                    let mut guard = worker.pending.lock().unwrap_or_else(|e| e.into_inner());
                    while guard.is_none() && !worker.closed.load(Ordering::Relaxed) {
                        guard = worker.wake.wait(guard).unwrap_or_else(|e| e.into_inner());
                    }
                    if worker.closed.load(Ordering::Relaxed) {
                        break;
                    }
                    guard.take()
                };
                let Some(Pending {
                    generation,
                    cancel,
                    job,
                }) = pending
                else {
                    continue;
                };
                let progress = |text| {
                    *worker.progress.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((generation, text));
                };
                let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                check(&cancel,Instant::now()+Duration::from_secs(MAX_SECONDS))?;
                match job {
                    Job::Scan(folder)=>scan(&folder,cancel.clone(),&progress).map(|v| Loaded::Catalog(Arc::new(v))),
                    Job::Demo(folder)=>crate::demo::ensure(&folder).and_then(|_| scan(&folder,cancel.clone(),&progress)).map(|v| Loaded::Catalog(Arc::new(v))),
                    Job::Load(catalog,request)=>load(&catalog,request,cancel.clone(),&progress).map(Loaded::Trends),
                }
            })).unwrap_or_else(|_| Err(t("Read stopped by an internal error. Retry with a verified archive.", "Lettura interrotta per un errore interno. Riprovare con un archivio verificato.").into()));
                if generation == worker.generation.load(Ordering::Relaxed) {
                    let result = if cancel.load(Ordering::Relaxed) {
                        Err(t("Loading cancelled.", "Caricamento annullato.").into())
                    } else {
                        result
                    };
                    *worker.output.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some((generation, result));
                }
            }
        });
        Self {
            shared,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
        }
    }
    pub fn submit(&mut self, job: Job) {
        self.cancel();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.shared
            .generation
            .store(self.generation, Ordering::Relaxed);
        *self.shared.output.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            generation: self.generation,
            cancel: self.cancel.clone(),
            job,
        });
        self.shared.wake.notify_one();
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn poll(&self) -> Option<Outcome<Loaded>> {
        self.shared
            .output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .filter(|(g, _)| *g == self.generation)
            .map(|(_, v)| v)
    }
    pub fn progress(&self) -> Option<String> {
        self.shared
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .filter(|(g, _)| *g == self.generation)
            .map(|(_, v)| v)
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.cancel();
        self.shared.closed.store(true, Ordering::Relaxed);
        self.shared.wake.notify_one();
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
