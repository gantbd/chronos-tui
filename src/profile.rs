//! Portable TUI connection configuration. Launch flags remain invocation-local.
use crate::Workstream;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_BYTES: u64 = 128 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u8,
    workstreams: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    directory: PathBuf,
}

pub fn load(path: &Path) -> Result<Vec<Workstream>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("workspace profile exceeds 128 KiB".into());
    }
    let profile: Profile = serde_json::from_slice(&bytes)?;
    if profile.version != 1 {
        return Err("unsupported workspace profile version".into());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let workstreams: Vec<_> = profile
        .workstreams
        .into_iter()
        .map(|entry| Workstream {
            name: entry.name,
            directory: if entry.directory.as_os_str().is_empty() {
                entry.directory
            } else {
                parent.join(entry.directory)
            },
        })
        .collect();
    crate::validate_workstreams(&workstreams)?;
    Ok(workstreams)
}

pub fn save(path: &Path, workstreams: &[Workstream]) -> Result<()> {
    crate::validate_workstreams(workstreams)?;
    let base = std::env::current_dir()?;
    let profile = Profile {
        version: 1,
        workstreams: workstreams
            .iter()
            .map(|entry| Entry {
                name: entry.name.clone(),
                directory: base.join(&entry.directory),
            })
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&profile)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BYTES {
        return Err("workspace profile exceeds 128 KiB".into());
    }
    use std::os::unix::fs::OpenOptionsExt;
    // Publish only a complete file and refuse an existing destination.
    let parent = path.parent().unwrap_or(Path::new("."));
    let staged = parent.join(format!(".chronos-profile-{}.tmp", std::process::id()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)?;
    let result = output
        .write_all(&bytes)
        .and_then(|()| output.sync_all())
        .and_then(|()| fs::hard_link(&staged, path));
    drop(output);
    let cleanup = fs::remove_file(&staged);
    result?;
    cleanup?;
    Ok(())
}
