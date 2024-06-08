use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
};

use lazy_static::lazy_static;
use regex::Regex;
use simple_logger::SimpleLogger;
use unrar::FileHeader;

lazy_static! {
    static ref RE_PART_FILE: Regex = Regex::new("part(\\d+).rar$").unwrap();
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
    queue: VecDeque<PathBuf>,
}

impl UnarchiveQueue {
    pub fn new() -> UnarchiveQueue {
        UnarchiveQueue { queue: VecDeque::new() }
    }

    pub fn find_rar_files(&mut self, root_dir: impl AsRef<Path>) -> anyhow::Result<()> {
        let pattern = root_dir.as_ref().join("**/*.rar");
        let pattern = pattern.to_string_lossy();
        for entry in glob::glob(&pattern)? {
            let entry = entry?;
            if is_root_rar_file(&entry) {
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
            archive.extract_into(dest)?;
        }

        for header in archive.headers {
            if is_root_rar_file(&header.filename) {
                log::info!("-> Archive contains archive '{}', enqueuing", header.filename.display());
                self.queue.push_back(dest.join(header.filename));
            }
        }

        Ok(())
    }
}

struct Archive {
    pub path: PathBuf,
    pub headers: Vec<FileHeader>,
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
        Ok(Archive { path, headers })
    }

    pub fn is_already_extracted(&self, dest: &Path) -> anyhow::Result<bool> {
        for header in self.headers.iter() {
            match fs::metadata(dest.join(&header.filename)) {
                Ok(md) => {
                    if md.len() != header.unpacked_size {
                        return Ok(false);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
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
}

fn main() -> anyhow::Result<()> {
    SimpleLogger::new().init().unwrap();

    let mut q = UnarchiveQueue::new();
    q.find_rar_files("/mnt/tank/video/tv-en/Bones")?;
    while q.process_next()? {}
    Ok(())
}
