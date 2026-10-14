#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
/// Formats the English or Italian text for the current UI language.
macro_rules! tr {
    ($en:literal, $it:literal $(, $arg:expr)* $(,)?) => {
        if $crate::italian() { format!($it $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}
mod archive;
mod demo;
mod gui;

use archive::{MAX_BUCKETS, Mode, Request};
use chrono::{Duration, NaiveDateTime};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// UI language: English by default, Italian when selected in the GUI.
static ITALIAN: AtomicBool = AtomicBool::new(false);
pub fn italian() -> bool {
    ITALIAN.load(Ordering::Relaxed)
}
pub fn set_italian(on: bool) {
    ITALIAN.store(on, Ordering::Relaxed);
}
/// Picks the English or Italian text for the current UI language.
pub fn t(en: &'static str, it: &'static str) -> &'static str {
    if italian() { it } else { en }
}

#[derive(Default)]
pub struct Options {
    folder: Option<PathBuf>,
    weekly: bool,
    days: Option<i64>,
    all: bool,
    tags: Vec<String>,
    start: Option<String>,
    end: Option<String>,
    check: bool,
    smoke: bool,
    screenshot: Option<PathBuf>,
    demo: bool,
}
fn options() -> Result<Options, String> {
    let mut result = Options::default();
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--data-dir" => result.folder=Some(args.next().ok_or("Missing folder.")?.into()),
            "--source" | "--smoke-source" => { let mode=args.next().ok_or("Missing source.")?; result.weekly=match mode.to_str() { Some("weekly")=>true,Some("daily")=>false,_=>return Err("Source: daily or weekly.".into()) }; }
            "--days" | "--smoke-days" => { let days=args.next().ok_or("Missing duration.")?.to_string_lossy().parse::<i64>().map_err(|_| "Invalid duration.")?; if !(1..=365000).contains(&days) { return Err("Duration: 1 to 365000 days.".into()); } result.days=Some(days); }
            "--all" => result.all=true,
            "--tags" => result.tags=args.next().ok_or("Missing tags.")?.to_string_lossy().split(',').map(str::to_string).collect(),
            "--start" => result.start=Some(args.next().ok_or("Missing date.")?.to_string_lossy().into_owned()),
            "--end" => result.end=Some(args.next().ok_or("Missing date.")?.to_string_lossy().into_owned()),
            "--check" => result.check=true,
            "--demo" => result.demo=true,
            "--smoke-test" => result.smoke=true,
            "--screenshot" => result.screenshot=Some(args.next().ok_or("Missing path.")?.into()),
            _ => return Err("Options: --data-dir FOLDER [--source daily|weekly] [--days N|--all] [--start DATE --end DATE] [--tags A,B] [--check|--smoke-test] [--screenshot FILE.png] [--demo]".into()),
        }
    }
    Ok(result)
}
pub fn date(text: &str) -> Result<NaiveDateTime, String> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").map_err(|_| {
        t(
            "Use the format YYYY-MM-DD HH:MM.",
            "Usare il formato AAAA-MM-GG HH:MM.",
        )
        .into()
    })
}
fn request(catalog: &archive::Catalog, opts: &Options) -> Result<Request, String> {
    let mode = if opts.weekly {
        Mode::Weekly
    } else {
        Mode::Daily
    };
    let (first, last) = catalog.bounds(mode)?;
    let end = opts.end.as_deref().map(date).transpose()?.unwrap_or(last);
    let start = if let Some(text) = opts.start.as_deref() {
        date(text)?
    } else if opts.all {
        first
    } else {
        end.checked_sub_signed(Duration::days(opts.days.unwrap_or(1)))
            .ok_or(t("Date out of range.", "Data fuori intervallo."))?
    };
    let tags = if opts.tags.is_empty() {
        catalog
            .tags
            .values()
            .filter(|t| t.kind == "Analog")
            .take(4)
            .map(|t| t.name.clone())
            .collect()
    } else {
        opts.tags.clone()
    };
    let request = Request {
        tags,
        start,
        end,
        mode,
        buckets: MAX_BUCKETS,
    };
    request.validate()?;
    Ok(request)
}
fn run() -> Result<(), String> {
    let mut opts = options()?;
    if opts.demo {
        // Synthetic archives: in --data-dir when given, otherwise in the temp folder.
        let folder = opts.folder.take().unwrap_or_else(demo::default_folder);
        demo::ensure(&folder)?;
        opts.folder = Some(folder);
    }
    if opts.check {
        let folder = opts.folder.as_ref().ok_or("--check requires --data-dir.")?;
        let cancel = Arc::new(AtomicBool::new(false));
        let catalog = archive::scan(folder, cancel.clone(), &|_| {})?;
        let trends = archive::load(&catalog, request(&catalog, &opts)?, cancel, &|_| {})?;
        let series:Vec<_>=trends.series.iter().map(|s|serde_json::json!({
            "tag":s.tag.name,"count":s.count,"valid":s.valid_count,
            "minimum":s.minimum.is_finite().then_some(s.minimum),"maximum":s.maximum.is_finite().then_some(s.maximum),
            "points":s.buckets.values().map(|b|b.points().len()).sum::<usize>()
        })).collect();
        println!(
            "{}",
            serde_json::json!({"archives":catalog.archives.len(),"series":series,"seconds":trends.seconds})
        );
        return Ok(());
    }
    gui::run(opts)
}
/// Load DLLs only from System32, so a planted opengl32.dll next to the exe is ignored.
#[cfg(windows)]
fn restrict_dll_search() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetDefaultDllDirectories(flags: u32) -> i32;
    }
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;
    // SAFETY: plain Win32 call with a constant flag, before any other thread starts.
    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
}
fn main() {
    #[cfg(windows)]
    restrict_dll_search();
    if let Err(error) = run() {
        eprintln!("{error}");
        // GUI launches have no console in release builds. Checks fail with a nonzero exit code.
        if !std::env::args().any(|a| a == "--check" || a == "--smoke-test") {
            rfd::MessageDialog::new()
                .set_title("Vessel Trend Manager")
                .set_description(&error)
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}
