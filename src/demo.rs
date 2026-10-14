//! Deterministic synthetic AMS archives for demos and screenshots. No real vessel data.
use crate::t;
use chrono::{Duration, NaiveDate, NaiveDateTime};
use rusqlite::{Connection, params};
use std::{
    f64::consts::TAU,
    fs,
    path::{Path, PathBuf},
};

/// Written last: its presence marks a complete demo folder.
const MARKER: &str = "SYNTHETIC_DEMO_DATA.txt";
/// Signals preselected when the demo opens.
pub const FEATURED: [&str; 4] = ["DG2-EXH", "DG1-LO-P", "ME-RPM", "FO-TANK"];
const GROUPS: [(i64, &str); 5] = [
    (1, "DIESEL GENERATOR 1"),
    (2, "DIESEL GENERATOR 2"),
    (3, "MAIN ENGINE"),
    (4, "COOLING WATER"),
    (5, "FUEL OIL"),
];
/// (name, description, unit, digital, alarm group); the TagConfigDynamic id is index + 1.
const TAGS: [(&str, &str, &str, bool, i64); 12] = [
    ("DG1-LOAD", "DG1 ACTIVE POWER", "kW", false, 1),
    ("DG1-EXH", "DG1 EXHAUST GAS TEMP", "°C", false, 1),
    ("DG1-LO-P", "DG1 LUB OIL PRESSURE", "bar", false, 1),
    ("DG1-RUN", "DG1 RUNNING", "", true, 1),
    ("DG2-LOAD", "DG2 ACTIVE POWER", "kW", false, 2),
    ("DG2-EXH", "DG2 EXHAUST GAS TEMP", "°C", false, 2),
    ("DG2-RUN", "DG2 RUNNING", "", true, 2),
    ("ME-RPM", "MAIN ENGINE SPEED", "rpm", false, 3),
    ("ME-EXH", "ME EXHAUST GAS TEMP AVG", "°C", false, 3),
    ("HT-CW-T", "HT COOLING WATER OUTLET TEMP", "°C", false, 4),
    ("SW-P", "SEA WATER PRESSURE", "bar", false, 4),
    ("FO-TANK", "FO SERVICE TANK LEVEL", "%", false, 5),
];
/// Synthetic machinery operating limits for the demo signals, in the user's limits file format.
pub const LIMITS: &str = "SYNTHETIC_DEMO_LIMITS.csv";
pub const LIMITS_CSV: &str =
    "# Synthetic machinery operating limits. Edit and save: the charts update.
tag;min;max;unit;notes
DG1-LOAD;;1000;kW;synthetic
DG2-LOAD;;1000;kW;synthetic
DG1-EXH;;480;°C;synthetic
DG2-EXH;;480;°C;synthetic, exceeded by the developing fault
DG1-LO-P;3.9;6;bar;synthetic, crossed by filter clogging
ME-RPM;;112;rpm;synthetic
ME-EXH;;420;°C;synthetic
HT-CW-T;70;90;°C;synthetic
SW-P;1.5;3;bar;synthetic
FO-TANK;20;;%;synthetic
";
const SCHEMA: &str = "CREATE TABLE TagType(Id INTEGER PRIMARY KEY,Name TEXT);
    INSERT INTO TagType VALUES(1,'Analog'),(2,'Digital');
    CREATE TABLE AlarmGroup(Id INTEGER PRIMARY KEY,AlarmGroupNumber INTEGER,AlarmGroupText TEXT);
    CREATE TABLE TagConfig(Id INTEGER PRIMARY KEY,Name TEXT,Description TEXT,EngUnit TEXT,TagType INTEGER,AlarmGroup INTEGER);
    CREATE TABLE TagConfigDynamic(Id INTEGER PRIMARY KEY,TagConfig INTEGER);";

fn day(y: i32, m: u32, d: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("valid demo date")
}
pub fn default_folder() -> PathBuf {
    std::env::temp_dir().join("vessel-trend-manager-demo")
}

/// Creates the demo archives in `folder` unless already complete. Other non-empty folders are refused.
pub fn ensure(folder: &Path) -> Result<(), String> {
    for parent in std::path::absolute(folder)
        .map_err(|e| e.to_string())?
        .ancestors()
    {
        if let Ok(meta) = fs::symlink_metadata(parent) {
            #[cfg(windows)]
            let redirected = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let redirected = meta.is_symlink();
            if redirected {
                return Err(t(
                    "Demo paths must not use links.",
                    "I percorsi demo non devono usare collegamenti.",
                )
                .into());
            }
        }
    }
    if folder.join(MARKER).is_file() {
        return Ok(());
    }
    let used = fs::read_dir(folder).is_ok_and(|mut d| d.next().is_some());
    if used {
        return Err(t(
            "Demo data needs an empty or new folder.",
            "I dati demo richiedono una cartella vuota o nuova.",
        )
        .into());
    }
    let write = || -> rusqlite::Result<()> {
        // Nine weeks of 10-minute aggregates, then four weeks of 2-minute daily readings.
        let mut week = day(2024, 12, 30);
        while week < day(2025, 3, 3) {
            let last = week + Duration::days(6);
            let name = format!(
                "weeks/Week_from_{}_to_{}.s3db",
                week.format("%Y-%m-%d"),
                last.format("%Y-%m-%d")
            );
            archive(
                &folder.join(name),
                true,
                week,
                week + Duration::days(7),
                600,
            )?;
            week += Duration::days(7);
        }
        let mut date = day(2025, 3, 3);
        while date < day(2025, 3, 31) {
            // One missing export shows how gaps are drawn.
            if date != day(2025, 3, 19) {
                let name = format!("days/Day_{}.s3db", date.format("%Y-%m-%d"));
                archive(
                    &folder.join(name),
                    false,
                    date,
                    date + Duration::days(1),
                    120,
                )?;
            }
            date += Duration::days(1);
        }
        Ok(())
    };
    fs::create_dir_all(folder.join("days")).map_err(|e| e.to_string())?;
    fs::create_dir_all(folder.join("weeks")).map_err(|e| e.to_string())?;
    write().map_err(|e| e.to_string())?;
    limits(folder)?;
    fs::write(
        folder.join(MARKER),
        "Synthetic archives generated by Vessel Trend Manager. No real vessel data.\n",
    )
    .map_err(|e| e.to_string())
}

/// Writes the demo limits once; later user edits to the file are kept.
fn limits(folder: &Path) -> Result<(), String> {
    let path = folder.join(LIMITS);
    if path.is_file() {
        return Ok(());
    }
    // The byte order mark makes Excel open the file as UTF-8.
    fs::write(path, ["\u{feff}", LIMITS_CSV].concat()).map_err(|e| e.to_string())
}

fn archive(
    path: &Path,
    weekly: bool,
    from: NaiveDateTime,
    to: NaiveDateTime,
    step: i64,
) -> rusqlite::Result<()> {
    let mut con = Connection::open(path)?;
    con.execute_batch(SCHEMA)?;
    let extra = if weekly {
        ",Min REAL,Max REAL,IntervalMarker INTEGER"
    } else {
        ""
    };
    con.execute_batch(&format!("CREATE TABLE TagValue(Id INTEGER PRIMARY KEY,Timestamp TEXT,Value REAL,IsInvalid INTEGER,IsOverride INTEGER,IsInAlarm INTEGER,IsInhibit INTEGER,TagConfigDynamic INTEGER{extra}); CREATE INDEX tag_time_idx ON TagValue(TagConfigDynamic,Timestamp); CREATE INDEX time_idx ON TagValue(Timestamp);"))?;
    let tx = con.transaction()?;
    for (number, text) in GROUPS {
        tx.execute(
            "INSERT INTO AlarmGroup(AlarmGroupNumber,AlarmGroupText) VALUES(?,?)",
            params![number, text],
        )?;
    }
    for (i, (name, description, unit, digital, group)) in TAGS.into_iter().enumerate() {
        let id = i as i64 + 1;
        tx.execute(
            "INSERT INTO TagConfig VALUES(?,?,?,?,?,?)",
            params![id, name, description, unit, 1 + i64::from(digital), group],
        )?;
        tx.execute("INSERT INTO TagConfigDynamic VALUES(?,?)", params![id, id])?;
    }
    {
        let mut insert = tx.prepare(if weekly {
            "INSERT INTO TagValue(Timestamp,Value,IsInvalid,IsOverride,IsInAlarm,IsInhibit,TagConfigDynamic,Min,Max,IntervalMarker) VALUES(?,?,?,0,?,?,?,?,?,10)"
        } else {
            "INSERT INTO TagValue(Timestamp,Value,IsInvalid,IsOverride,IsInAlarm,IsInhibit,TagConfigDynamic) VALUES(?,?,?,0,?,?,?)"
        })?;
        let mut at = from;
        while at < to {
            let stamp = at.format("%Y-%m-%d %H:%M:%S").to_string();
            for (i, (_, _, _, digital, _)) in TAGS.into_iter().enumerate() {
                let s = sample(i, at);
                let id = i as i64 + 1;
                if weekly {
                    let spread = if digital {
                        0.0
                    } else {
                        (s.value.abs() * 0.015).max(0.05) * (1.0 + noise(i + 50, at).abs())
                    };
                    insert.execute(params![
                        stamp,
                        s.value,
                        s.invalid,
                        s.alarm,
                        s.inhibit,
                        id,
                        s.value - spread,
                        s.value + spread
                    ])?;
                } else {
                    insert.execute(params![stamp, s.value, s.invalid, s.alarm, s.inhibit, id])?;
                }
            }
            at += Duration::seconds(step);
        }
    }
    tx.commit()
}

struct Sample {
    value: f64,
    invalid: bool,
    alarm: bool,
    inhibit: bool,
}
/// Deterministic noise in [-1, 1) (SplitMix64), so every demo build is identical.
fn noise(tag: usize, at: NaiveDateTime) -> f64 {
    let mut x = (at.and_utc().timestamp() as u64) ^ ((tag as u64 + 1) << 48);
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}
/// Voyage of 5 days at sea and 2 in port, with a developing DG2 exhaust fault,
/// DG1 lube oil filter clogging, a sea water pressure sensor dropout and a DG2 inhibit.
fn sample(tag: usize, at: NaiveDateTime) -> Sample {
    let days = |t: NaiveDateTime| (at - t).num_seconds() as f64 / 86_400.0;
    let elapsed = days(day(2024, 12, 30));
    let sea = (elapsed as i64).rem_euclid(7) < 5;
    let n = noise(tag, at);
    let wave = (elapsed * TAU).sin();
    let demand = if sea {
        900.0 + 150.0 * wave
    } else {
        520.0 + 120.0 * wave
    };
    let load = if sea { demand / 2.0 } else { demand };
    let rpm = 104.0 + 4.0 * (elapsed * TAU / 0.3).sin() + 1.2 * n;
    let within = |start: NaiveDateTime, hours: f64| (0.0..hours / 24.0).contains(&days(start));
    let mut s = Sample {
        value: 0.0,
        invalid: false,
        alarm: false,
        inhibit: false,
    };
    s.value = match tag {
        0 => load * (1.0 + 0.03 * n),
        1 => 250.0 + 0.2 * load + 6.0 * n,
        2 => 4.6 - 0.04 * elapsed.rem_euclid(21.0) + 0.03 * n,
        3 => 1.0,
        4 if sea => load * (1.0 + 0.03 * n),
        5 if sea => 250.0 + 0.2 * load + 6.0 * n + 18.0 * days(day(2025, 3, 20)).max(0.0),
        5 => 45.0 + 2.0 * n,
        6 => f64::from(u8::from(sea)),
        7 if sea => rpm,
        8 if sea => 300.0 + 0.6 * rpm + 5.0 * n,
        8 => 55.0 + 2.0 * n,
        9 => 80.0 + if sea { 3.0 } else { 0.0 } + 1.2 * wave + 0.4 * n,
        10 => 2.2 + 0.05 * n,
        11 => 95.0 - 60.0 * (elapsed * 24.0 / 30.0).fract() + 0.3 * n,
        _ => 0.0,
    };
    s.alarm = match tag {
        1 | 5 => s.value > 480.0,
        2 => s.value < 3.9,
        _ => false,
    };
    s.inhibit = tag == 5 && within(day(2025, 3, 8) + Duration::hours(6), 4.0);
    if tag == 10 && within(day(2025, 3, 12) + Duration::hours(8), 6.0) {
        s.value = 0.0;
        s.invalid = true;
    }
    s
}
