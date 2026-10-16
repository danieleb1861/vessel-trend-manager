//! Synthetic archives only. No vessel exports or operational fixtures.
use super::*;

struct Fixture {
    folder: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = std::env::temp_dir().join(format!(
            "vessel_trend_test_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        let f = Self { folder };
        f.db(
            "Day_2026-09-01.s3db",
            false,
            "INSERT INTO TagValue VALUES
            (1,'2026-08-28 12:00:00.0000001',20,0,0,0,0,7),
            (2,'2026-08-28 12:00:00.0000001',1,0,0,0,0,8),
            (3,'2026-09-01 00:00:00',10,0,0,0,0,7),
            (4,'2026-09-01 00:00:01.1234567',100,0,0,1,0,7),
            (5,'2026-09-01 00:00:02',-1000,1,0,0,0,7),
            (6,'2026-09-01 00:00:03',999,0,1,0,0,7),
            (7,'2026-09-01 00:00:04',-5,0,0,0,1,7),
            (8,'2026-09-01 23:59:59.9999999',11,NULL,NULL,NULL,NULL,7);",
        );
        f.db(
            "Day_2026-09-02.s3db",
            false,
            "INSERT INTO TagValue VALUES
            (1,'2026-09-01 23:59:59.9999999',11,0,0,0,0,7),
            (2,'2026-08-28 12:00:00.0000001',1,0,0,0,0,8),
            (3,'2026-09-02 00:00:00',12,0,0,0,0,7);",
        );
        f.db(
            "Week_from_2026-08-31_to_2026-09-06.s3db",
            true,
            "INSERT INTO TagValue VALUES
            (1,'2026-09-01 00:02:00',25,0,0,0,0,7,10,40,5),
            (2,'2026-09-01 00:07:00',30,0,0,0,0,7,-20,95,10),
            (3,'2026-09-01 00:27:00',28,0,0,0,0,7,20,35,30);",
        );
        f
    }
    fn db(&self, name: &str, weekly: bool, insert: &str) {
        let con = Connection::open(self.folder.join(name)).unwrap();
        con.execute_batch("CREATE TABLE TagType(Id INTEGER PRIMARY KEY,Name TEXT); INSERT INTO TagType VALUES(1,'Analog'),(2,'Digital');
            CREATE TABLE AlarmGroup(Id INTEGER,AlarmGroupNumber INTEGER,AlarmGroupText TEXT); INSERT INTO AlarmGroup VALUES(99,11,'ENGINE A');
            CREATE TABLE TagConfig(Id INTEGER PRIMARY KEY,Name TEXT,Description TEXT,EngUnit TEXT,TagType INTEGER,AlarmGroup INTEGER);
            INSERT INTO TagConfig VALUES(1,'TEMP','TEST TEMPERATURE','C',1,11),(2,'RUN','RUNNING','',2,11);
            CREATE TABLE TagConfigDynamic(Id INTEGER PRIMARY KEY,TagConfig INTEGER); INSERT INTO TagConfigDynamic VALUES(7,1),(8,2);").unwrap();
        let extra = if weekly {
            ",Min REAL,Max REAL,IntervalMarker INTEGER"
        } else {
            ""
        };
        con.execute_batch(&format!("CREATE TABLE TagValue(Id INTEGER PRIMARY KEY,Timestamp TEXT,Value REAL,IsInvalid INTEGER,IsOverride INTEGER,IsInAlarm INTEGER,IsInhibit INTEGER,TagConfigDynamic INTEGER{extra}); CREATE INDEX time_idx ON TagValue(Timestamp); CREATE INDEX tag_time_idx ON TagValue(TagConfigDynamic,Timestamp); {insert}")).unwrap();
    }
    fn catalog(&self) -> Catalog {
        scan(&self.folder, cancel(), &|_| {}).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}
fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
fn request() -> Request {
    Request {
        tags: vec!["TEMP".into()],
        start: timestamp("2026-09-01 00:00:00").unwrap(),
        end: timestamp("2026-09-03 00:00:00").unwrap(),
        mode: Mode::Daily,
        buckets: 1,
    }
}
fn result(f: &Fixture, r: Request) -> Trends {
    load(&f.catalog(), r, cancel(), &|_| {}).unwrap()
}

#[test]
fn catalog_uses_group_number_and_distinguishes_sources() {
    let f = Fixture::new();
    let c = f.catalog();
    assert_eq!(c.tags["TEMP"].group, "ENGINE A");
    assert_eq!(c.available(Mode::Daily).count(), 2);
    assert_eq!(c.available(Mode::Weekly).count(), 1);
}
#[test]
fn boundaries_quality_extremes_and_old_seeds() {
    let f = Fixture::new();
    let t = result(&f, request());
    let s = &t.series[0];
    assert_eq!((s.count, s.valid_count), (7, 5));
    assert_eq!((s.minimum, s.maximum), (-5.0, 100.0));
    assert_eq!(s.last.as_ref().unwrap().value, 12.0);
    assert_eq!(s.previous.as_ref().unwrap().value, 20.0);
    let b = &s.buckets[&0];
    assert_eq!((b.bad, b.alarm, b.inhibit), (2, 1, 1));
    assert_eq!(
        b.points().iter().map(|p| p.value).collect::<Vec<_>>(),
        vec![10.0, 100.0, -5.0, 12.0]
    );
    assert_eq!(b.points()[1].stamp, "2026-09-01 00:00:01.1234567");
    // Every daily reading in the period is kept raw, by time, with its quality flags and row.
    assert!(t.raw);
    assert_eq!(s.raw.len(), s.count);
    assert_eq!(s.raw.iter().filter(|r| r.good()).count(), s.valid_count);
    assert!(s.raw.windows(2).all(|w| w[0].x <= w[1].x));
    assert_eq!(
        s.raw.iter().map(|r| r.flags >> 1 & 1).sum::<u8>() as usize,
        b.alarm
    );
    assert!(
        s.raw
            .iter()
            .all(|r| t.sources[r.source as usize].exists() && r.row_id > 0)
    );
    let mut weekly = request();
    weekly.mode = Mode::Weekly;
    let w = result(&f, weekly);
    assert!(!w.raw && w.series[0].raw.is_empty());
}
#[test]
fn fixed_timestamp_layout_matches_the_generic_parser() {
    for text in [
        "2026-09-01 00:00:01",
        "2026-09-01 00:00:01.1234567",
        "2026-09-01 23:59:59.5",
        "2026-12-31 12:34:56.123456789",
        "2024-02-29 08:00:00.000",
    ] {
        let generic = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f").unwrap();
        assert_eq!(timestamp(text).unwrap(), generic, "{text}");
    }
    // Unusual layouts fall back to the generic parser, which keeps its own rules.
    for text in [
        "2026-09-01 00:00:60",
        "2026-09-01 00:00:01.1234567891",
        "2026-02-30 00:00:00",
        "2026-09-01T00:00:00",
        "2026-09-01 00:00:01.",
        "x",
    ] {
        assert_eq!(
            timestamp(text).ok(),
            NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f").ok(),
            "{text}"
        );
    }
}
#[test]
fn wider_period_reads_only_what_memory_lacks_like_a_disk_read() {
    let f = Fixture::new();
    let c = f.catalog();
    let disk = result(&f, request());
    let rows = |t: &Trends| {
        t.series[0]
            .raw
            .iter()
            .map(|r| {
                (
                    r.time(),
                    r.row_id,
                    r.value.to_bits(),
                    r.flags,
                    t.sources[r.source as usize].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let key = |p: &Option<Sample>| p.as_ref().map(|p| (p.at, p.row_id, p.stamp.clone()));
    // Same start (whole first archive in memory) and later start (previous record re-read).
    for (start, end) in [
        ("2026-09-01 00:00:00", "2026-09-02 00:00:00"),
        ("2026-09-01 00:00:01", "2026-09-01 12:00:00"),
    ] {
        let mut part = request();
        part.start = timestamp(start).unwrap();
        part.end = timestamp(end).unwrap();
        let known = result(&f, part);
        let wide = extend(&c, request(), &known, cancel(), &|_| {}).unwrap();
        let (d, w) = (&disk.series[0], &wide.series[0]);
        assert!(wide.raw && wide.reused == known.series[0].count && wide.reused > 0);
        assert_eq!((w.count, w.valid_count), (d.count, d.valid_count));
        assert_eq!((w.minimum, w.maximum), (d.minimum, d.maximum));
        assert_eq!(rows(&wide), rows(&disk));
        assert_eq!(key(&w.last), key(&d.last));
        assert_eq!(key(&w.previous), key(&d.previous));
        assert_eq!(wide.coverage, disk.coverage);
    }
    // A load that does not fit (other signals) is read whole.
    let mut other = request();
    other.tags = vec!["RUN".into()];
    let known = result(&f, other);
    let wide = extend(&c, request(), &known, cancel(), &|_| {}).unwrap();
    assert_eq!((wide.reused, rows(&wide)), (0, rows(&disk)));
}
#[test]
fn time_limit_follows_progress_within_a_total_cap() {
    let ms = Duration::from_millis;
    let idle = Deadline::new(ms(300), Duration::from_secs(60));
    std::thread::sleep(ms(200));
    idle.progress();
    std::thread::sleep(ms(200));
    assert!(check(&AtomicBool::new(false), &idle).is_ok());
    std::thread::sleep(ms(250));
    assert!(check(&AtomicBool::new(false), &idle).is_err());
    let total = Deadline::new(Duration::from_secs(60), ms(100));
    std::thread::sleep(ms(150));
    total.progress();
    assert!(check(&AtomicBool::new(false), &total).is_err());
}
#[test]
fn shorter_period_is_served_from_memory_like_a_disk_read() {
    let f = Fixture::new();
    let full = result(&f, request());
    let mut short = request();
    short.start = timestamp("2026-09-01 00:00:01").unwrap();
    short.end = timestamp("2026-09-02 00:00:00").unwrap();
    let disk = result(&f, short.clone());
    let memory = narrow(&full, &short).unwrap();
    let (d, m) = (&disk.series[0], &memory.series[0]);
    assert!(memory.raw);
    assert_eq!((m.count, m.valid_count), (d.count, d.valid_count));
    assert_eq!((m.minimum, m.maximum), (d.minimum, d.maximum));
    assert_eq!(
        m.raw
            .iter()
            .map(|r| (r.row_id, r.value.to_bits()))
            .collect::<Vec<_>>(),
        d.raw
            .iter()
            .map(|r| (r.row_id, r.value.to_bits()))
            .collect::<Vec<_>>()
    );
    let key = |p: &Option<Sample>| p.as_ref().map(|p| (p.at, p.row_id, p.value.to_bits()));
    assert_eq!(key(&m.last), key(&d.last));
    assert_eq!(key(&m.previous), key(&d.previous));
    assert_eq!(memory.coverage, disk.coverage);
    // Archive times are kept to the nanosecond, with the end excluded as on disk.
    let mut fine = request();
    fine.start = timestamp("2026-09-01 00:00:01.1234566").unwrap();
    fine.end = timestamp("2026-09-01 00:00:02").unwrap();
    assert_eq!(narrow(&full, &fine).unwrap().series[0].count, 1);
    fine.end = timestamp("2026-09-01 00:00:01.1234567").unwrap();
    assert_eq!(narrow(&full, &fine).unwrap().series[0].count, 0);
    // Wider periods, other sources and signals not loaded go to disk.
    let mut wider = request();
    wider.end = timestamp("2026-09-04 00:00:00").unwrap();
    let mut weekly = short.clone();
    weekly.mode = Mode::Weekly;
    let mut other = short.clone();
    other.tags = vec!["RUN".into()];
    for r in [wider, weekly, other] {
        assert!(narrow(&full, &r).is_none());
    }
}
#[test]
fn nanosecond_precision_and_exclusive_end() {
    let f = Fixture::new();
    let mut r = request();
    r.start = timestamp("2026-09-01 00:00:01.1234566").unwrap();
    r.end = timestamp("2026-09-01 00:00:02").unwrap();
    assert_eq!(result(&f, r.clone()).series[0].count, 1);
    r.end = timestamp("2026-09-01 00:00:01.1234567").unwrap();
    assert_eq!(result(&f, r).series[0].count, 0);
}
#[test]
fn missing_digital_state_is_never_invented() {
    let f = Fixture::new();
    let mut r = request();
    r.tags = vec!["RUN".into()];
    let t = result(&f, r);
    assert_eq!(t.series[0].count, 0);
    assert!(t.series[0].last.is_none());
    assert_eq!(t.series[0].previous.as_ref().unwrap().value, 1.0);
}
#[test]
fn weekly_extrema_and_all_interval_markers() {
    let f = Fixture::new();
    let mut r = request();
    r.mode = Mode::Weekly;
    r.buckets = 800;
    let t = result(&f, r);
    let s = &t.series[0];
    assert_eq!(s.count, 3);
    assert_eq!((s.minimum, s.maximum), (-20.0, 95.0));
    assert_eq!(s.last.as_ref().unwrap().marker.as_deref(), Some("30"));
}
#[test]
fn arbitrarily_long_windows_remain_bounded() {
    let f = Fixture::new();
    f.db(
        "Day_2026-11-01.s3db",
        false,
        "INSERT INTO TagValue VALUES(1,'2026-11-01 12:00:00',150,0,0,0,0,7);",
    );
    let mut r = request();
    r.end = timestamp("2026-11-02 00:00:00").unwrap();
    let t = result(&f, r.clone());
    assert_eq!(t.series[0].count, 8);
    assert_eq!(t.series[0].maximum, 150.0);
    assert!(t.series[0].buckets.len() <= r.buckets);
    r.mode = Mode::Weekly;
    r.start = timestamp("2025-01-01 00:00:00").unwrap();
    r.end = timestamp("2027-01-01 00:00:00").unwrap();
    assert_eq!(result(&f, r).series[0].count, 3);
}
#[test]
fn raw_database_is_unchanged_and_read_only() {
    // Minimum bundled SQLite with the reviewed native security fixes.
    assert!(rusqlite::version_number() >= 3_053_002);
    println!("SQLite runtime: {}", rusqlite::version());
    let f = Fixture::new();
    let p = f.folder.join("Day_2026-09-01.s3db");
    let before = fs::read(&p).unwrap();
    result(&f, request());
    let con = connect(&p, cancel(), &deadline()).unwrap();
    assert!(con.execute("DELETE FROM TagValue", []).is_err());
    assert!(
        con.query_row("PRAGMA hard_heap_limit", [], |r| r.get::<_, i64>(0))
            .unwrap()
            <= 67_108_864
    );
    assert_eq!(
        con.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(con);
    assert_eq!(before, fs::read(&p).unwrap());
}
#[test]
fn wal_archive_and_unusual_path_leave_no_side_files() {
    let f = Fixture::new();
    let folder = f.folder.join("odd #1 %20 Ã¨");
    fs::create_dir(&folder).unwrap();
    for name in [
        "Day_2026-09-01.s3db",
        "Day_2026-09-02.s3db",
        "Week_from_2026-08-31_to_2026-09-06.s3db",
    ] {
        fs::copy(f.folder.join(name), folder.join(name)).unwrap();
    }
    let con = Connection::open(folder.join("Day_2026-09-01.s3db")).unwrap();
    let mode: String = con
        .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    drop(con);
    let c = scan(&folder, cancel(), &|_| {}).unwrap();
    assert!(c.warnings.is_empty());
    assert_eq!(
        load(&c, request(), cancel(), &|_| {}).unwrap().series[0].count,
        7
    );
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 3);
    // A pending WAL would be skipped by an immutable read: the archive is refused.
    fs::write(folder.join("Day_2026-09-02.s3db-wal"), b"synthetic").unwrap();
    let c = scan(&folder, cancel(), &|_| {}).unwrap();
    assert_eq!((c.archives.len(), c.warnings.len()), (2, 1));
}
#[test]
fn changed_archive_requires_rescan() {
    let f = Fixture::new();
    let c = f.catalog();
    let con = Connection::open(f.folder.join("Day_2026-09-01.s3db")).unwrap();
    con.execute_batch("CREATE TABLE SyntheticChange(id INTEGER);")
        .unwrap();
    drop(con);
    assert!(
        load(&c, request(), cancel(), &|_| {})
            .unwrap_err()
            .contains("changed")
    );
}
#[test]
fn incompatible_historical_units_fail_closed() {
    let f = Fixture::new();
    let con = Connection::open(f.folder.join("Day_2026-09-01.s3db")).unwrap();
    con.execute("UPDATE TagConfig SET EngUnit='K' WHERE Name='TEMP'", [])
        .unwrap();
    drop(con);
    assert!(
        load(&f.catalog(), request(), cancel(), &|_| {})
            .unwrap_err()
            .contains("unit or type")
    );
}
#[test]
fn invalid_file_is_reported_without_losing_valid_archives() {
    let f = Fixture::new();
    fs::write(
        f.folder.join("Day_2026-10-01.s3db"),
        b"synthetic corrupt database",
    )
    .unwrap();
    let c = f.catalog();
    assert_eq!(c.archives.len(), 3);
    assert!(!c.warnings.is_empty());
}
#[test]
fn cancellation_and_sqlite_deadline_interrupt_work() {
    let f = Fixture::new();
    let token = Arc::new(AtomicBool::new(true));
    assert!(
        scan(&f.folder, token, &|_| {})
            .unwrap_err()
            .contains("cancelled")
    );
    let short = Duration::from_millis(10);
    let con = connect(
        &f.folder.join("Day_2026-09-01.s3db"),
        cancel(),
        &Deadline::new(short, short),
    )
    .unwrap();
    let start = Instant::now();
    assert!(con.query_row("SELECT COUNT(*) FROM TagValue a,TagValue b,TagValue c,TagValue d,TagValue e,TagValue f,TagValue g,TagValue h",[],|r|r.get::<_,i64>(0)).is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
}
#[test]
fn reduction_keeps_extreme_spike_without_growing() {
    let f = Fixture::new();
    let mut s = Series::new(f.catalog().tags["TEMP"].clone());
    let template = result(&f, request()).series[0].last.clone().unwrap();
    for i in 0..20_000 {
        let mut p = template.clone();
        p.row_id = i;
        p.at += Span::microseconds(i);
        p.value = if i == 4321 { 9999.0 } else { i as f64 % 100.0 };
        p.low = p.value;
        p.high = p.value;
        s.add(p, 0);
    }
    assert_eq!(s.count, 20_000);
    assert_eq!(s.maximum, 9999.0);
    assert!(s.buckets[&0].points().len() <= 4);
    assert!(s.buckets[&0].points().iter().any(|p| p.value == 9999.0));
}
#[test]
fn request_validation_rejects_duplicate_tags_and_empty_ranges() {
    let mut r = request();
    r.tags = vec!["TEMP".into(), "TEMP".into()];
    assert!(r.validate().is_err());
    r.tags = vec!["TEMP".into()];
    r.end = r.start;
    assert!(r.validate().is_err());
}

#[test]
fn untrusted_metadata_and_views_are_rejected() {
    let f = Fixture::new();
    let path = f.folder.join("Day_2026-09-01.s3db");
    let con = Connection::open(&path).unwrap();
    con.execute(
        "UPDATE TagConfig SET Description=?",
        ["x".repeat(MAX_TEXT_BYTES + 1)],
    )
    .unwrap();
    drop(con);
    let c = f.catalog();
    assert_eq!(c.available(Mode::Daily).count(), 1);
    assert_eq!(c.warnings.len(), 1);
    let con = Connection::open(&path).unwrap();
    con.execute_batch("DROP TABLE TagConfig; CREATE VIEW TagConfig AS SELECT 1 AS Id;")
        .unwrap();
    drop(con);
    assert!(connect(&path, cancel(), &deadline()).is_err());
}

#[test]
fn catalog_and_dynamic_id_caps_fail_closed() {
    let f = Fixture::new();
    let path = f.folder.join("Day_2026-09-01.s3db");
    let con = Connection::open(&path).unwrap();
    con.execute("WITH RECURSIVE n(i) AS (SELECT 10 UNION ALL SELECT i+1 FROM n WHERE i<?) INSERT INTO TagConfig SELECT i,'TEST-'||i,'SYNTHETIC','C',1,11 FROM n", [MAX_CATALOG_TAGS as i64 + 10]).unwrap();
    drop(con);
    assert_eq!(f.catalog().available(Mode::Daily).count(), 1);
    let con = Connection::open(&path).unwrap();
    con.execute_batch("DELETE FROM TagConfig WHERE Id>=10;")
        .unwrap();
    con.execute("WITH RECURSIVE n(i) AS (SELECT 10 UNION ALL SELECT i+1 FROM n WHERE i<?) INSERT INTO TagConfigDynamic SELECT i,1 FROM n", [MAX_DYNAMIC_IDS as i64 + 10]).unwrap();
    drop(con);
    assert!(load(&f.catalog(), request(), cancel(), &|_| {}).is_err());
}

fn wait(manager: &Manager) -> Outcome<Loaded> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = manager.poll() {
            return value;
        }
        assert!(Instant::now() < deadline, "Worker timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn worker_latest_request_wins_and_recovers_after_error() {
    let f = Fixture::new();
    let mut manager = Manager::new();
    for _ in 0..30 {
        manager.submit(Job::Scan(f.folder.join("missing")));
    }
    manager.submit(Job::Scan(f.folder.clone()));
    let Loaded::Catalog(c) = wait(&manager).unwrap() else {
        panic!("catalog expected")
    };
    let mut bad = request();
    bad.tags = vec!["UNKNOWN".into()];
    manager.submit(Job::Load(c.clone(), bad, None));
    assert!(wait(&manager).is_err());
    manager.submit(Job::Load(c, request(), None));
    let Loaded::Trends(t) = wait(&manager).unwrap() else {
        panic!("trends expected")
    };
    assert_eq!(t.series[0].count, 7);
}
#[test]
fn synthetic_demo_builds_loads_and_refuses_other_folders() {
    let f = Fixture::new();
    let folder = f.folder.join("demo");
    crate::demo::ensure(&folder).unwrap();
    // A complete demo folder is reused as is; edited demo limits are kept.
    fs::write(folder.join(crate::demo::LIMITS), "edited").unwrap();
    crate::demo::ensure(&folder).unwrap();
    assert_eq!(
        fs::read_to_string(folder.join(crate::demo::LIMITS)).unwrap(),
        "edited"
    );
    let c = scan(&folder, cancel(), &|_| {}).unwrap();
    assert_eq!(c.available(Mode::Daily).count(), 27);
    assert_eq!(c.available(Mode::Weekly).count(), 9);
    assert!(c.warnings.is_empty());
    let (_, end) = c.bounds(Mode::Daily).unwrap();
    let request = Request {
        tags: crate::demo::FEATURED
            .iter()
            .map(|s| s.to_string())
            .collect(),
        start: end - Span::days(7),
        end,
        mode: Mode::Daily,
        buckets: MAX_BUCKETS,
    };
    let t = load(&c, request, cancel(), &|_| {}).unwrap();
    assert!(t.series.iter().all(|s| s.valid_count > 0));
    // The developing DG2 exhaust fault reaches the alarm limit.
    assert!(t.series[0].buckets.values().any(|b| b.alarm > 0));
    // A non-empty folder that is not a demo is never written.
    assert!(crate::demo::ensure(&f.folder).is_err());
}
