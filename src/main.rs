use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use clap::Parser;
use lazy_static::lazy_static;
use regex::Regex;
use simple_logger::SimpleLogger;
use time::{
    format_description::{self, OwnedFormatItem},
    OffsetDateTime,
};
use unrar::FileHeader;

lazy_static! {
    static ref RE_PART_FILE: Regex = Regex::new("part(\\d+).rar$").unwrap();
    static ref TIME_FORMAT: OwnedFormatItem = format_description::parse_owned::<2>("[year]-[month]-[day]").unwrap();
}

fn is_root_rar_file(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|s| s.to_str()).expect("invalid file_name");
    if let Some(caps) = RE_PART_FILE.captures(file_name) {
        let part_num = u64::from_str_radix(&caps[1], 10).unwrap();
        return part_num == 1;
    }
    file_name.ends_with(".rar")
}

pub struct UnarchiveQueue {
    dry_run: bool,
    remove_after: Option<Duration>,
    queue: VecDeque<PathBuf>,
}

impl UnarchiveQueue {
    pub fn new(dry_run: bool, remove_after: Option<Duration>) -> UnarchiveQueue {
        UnarchiveQueue {
            dry_run,
            remove_after,
            queue: VecDeque::new(),
        }
    }

    pub fn find_rar_files(&mut self, root_dir: impl AsRef<Path>) -> anyhow::Result<()> {
        log::info!("Scanning for .rar files in '{}'", root_dir.as_ref().display());
        let pattern = root_dir.as_ref().join("**/*.rar");
        let pattern = pattern.to_string_lossy();
        for entry in glob::glob(&pattern)? {
            let entry = entry?;
            if is_root_rar_file(&entry) {
                log::debug!("'{}' enqueued.", entry.display());
                self.queue.push_back(entry);
            }
        }
        Ok(())
    }

    pub fn process_next(&mut self) -> anyhow::Result<bool> {
        match self.queue.pop_front() {
            None => Ok(false),
            Some(entry) => {
                self.process_entry(entry)?;
                Ok(true)
            }
        }
    }

    fn process_entry(&mut self, entry: PathBuf) -> anyhow::Result<()> {
        log::info!("Analyzing '{}'.", entry.display());
        let archive = Archive::open(entry)?;
        let dest = archive.path.as_path().parent().unwrap();

        if archive.is_already_extracted(dest)? {
            log::info!("-> Archive already extracted.");
        } else {
            log::info!("-> Extracting into '{}'.", dest.display());
            if !self.dry_run {
                archive.extract_into(dest)?;
            }
        }

        for header in &archive.headers {
            if is_root_rar_file(&header.filename) {
                log::info!("-> Archive contains archive '{}', enqueuing", header.filename.display());
                self.queue.push_back(dest.join(&header.filename));
            }
        }

        if let Some(remove_after) = self.remove_after {
            for entry in archive.list_parts()? {
                let md = entry.metadata()?;
                let mtime = md.modified()?;
                let elapsed = mtime.elapsed().unwrap_or(Duration::from_millis(0));
                if elapsed > remove_after {
                    log::info!(
                        "-> Removing archive/part '{}' last modified on '{}'.",
                        entry.display(),
                        OffsetDateTime::from(mtime)
                            .format(&TIME_FORMAT)
                            .unwrap_or_else(|_| "Unknown".into()),
                    );
                    if !self.dry_run {
                        fs::remove_file(entry)?;
                    }
                }
            }
        }

        Ok(())
    }
}

struct Archive {
    pub path: PathBuf,
    pub headers: Vec<FileHeader>,
    pub parts_glob: PathBuf,
}

impl Archive {
    pub fn open(path: impl Into<PathBuf>) -> anyhow::Result<Archive> {
        let path = path.into();
        let mut headers = Vec::new();
        let archive = unrar::Archive::new(&path).open_for_listing()?;
        for header in archive {
            let header = header?;
            headers.push(header);
        }

        Ok(Archive {
            parts_glob: unrar::Archive::new(&path).all_parts(),
            path,
            headers,
        })
    }

    pub fn is_already_extracted(&self, dest: &Path) -> anyhow::Result<bool> {
        for header in self.headers.iter() {
            match fs::metadata(dest.join(&header.filename)) {
                Ok(md) => {
                    if md.len() != header.unpacked_size {
                        log::debug!(
                            "'{}' size mismatch, got {} want {}",
                            header.filename.display(),
                            header.unpacked_size,
                            md.len()
                        );
                        return Ok(false);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    log::debug!("'{}' not found in destination", header.filename.display());
                    return Ok(false);
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(true)
    }

    pub fn extract_into(&self, dest: &Path) -> anyhow::Result<()> {
        let mut archive = unrar::Archive::new(&self.path).open_for_processing()?;
        while let Some(header) = archive.read_header()? {
            archive = if header.entry().is_file() {
                header.extract_with_base(dest)?
            } else {
                header.skip()?
            };
        }
        Ok(())
    }

    pub fn list_parts(&self) -> anyhow::Result<Vec<PathBuf>> {
        let pattern = &self.parts_glob.to_string_lossy();
        let mut results = Vec::new();
        for entry in glob::glob(pattern)? {
            let entry = entry?;
            results.push(entry);
        }
        Ok(results)
    }
}

#[derive(Parser, Debug)]
struct Args {
    root_dir: String,
    #[arg(long, default_value = "info")]
    log_level: log::LevelFilter,
    #[arg(long, default_value = "false")]
    dry_run: bool,
    #[arg(long)]
    remove_after_hours: Option<u64>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    SimpleLogger::new()
        .with_level(args.log_level)
        .init()
        .expect("unable to install logging");

    let mut q = UnarchiveQueue::new(
        args.dry_run,
        args.remove_after_hours.map(|h| Duration::from_secs(60 * 60 * 24 * h)),
    );
    q.find_rar_files(args.root_dir)?;
    while q.process_next()? {}
    Ok(())
}
