//! Integration-owned civil dates. These rows carry no scheduler authority.
use serde::Deserialize;
use std::{fs::File, io::Read, path::Path};

const MAX_BYTES: u64 = 128 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    version: u8,
    items: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Item {
    pub(crate) label: String,
    pub(crate) workstream: String,
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    pub(crate) due: Option<String>,
}

pub(crate) struct Plan {
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
}

impl Plan {
    pub(crate) fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("planning source exceeds 128 KiB".into());
        }
        let source: Source = serde_json::from_slice(&bytes)?;
        if source.version != 1 || source.items.len() > 256 {
            return Err("planning source requires version 1 and at most 256 items".into());
        }
        for item in &source.items {
            if [&item.label, &item.workstream].iter().any(|value| {
                value.trim().is_empty() || value.len() > 96 || value.chars().any(char::is_control)
            }) {
                return Err("planning labels must contain 1–96 printable bytes".into());
            }
            for date in [&item.start, &item.end, &item.due].into_iter().flatten() {
                if day(date).is_none() {
                    return Err("planning dates must be valid YYYY-MM-DD civil dates".into());
                }
            }
            if matches!((&item.start, &item.end), (Some(start), Some(end)) if start > end) {
                return Err("planned start follows planned end".into());
            }
        }
        Ok(Self {
            items: source.items,
            selected: 0,
        })
    }

    pub(crate) fn range(&self) -> Option<(&str, &str)> {
        let complete = self
            .items
            .iter()
            .filter_map(|item| Some((item.start.as_deref()?, item.end.as_deref()?)));
        Some((
            complete.clone().map(|(start, _)| start).min()?,
            complete.map(|(_, end)| end).max()?,
        ))
    }
}

pub(crate) fn day(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, b)| i != 4 && i != 7 && !b.is_ascii_digit())
    {
        return None;
    }
    let year: usize = value[..4].parse().ok()?;
    let month: usize = value[5..7].parse().ok()?;
    let date: usize = value[8..].parse().ok()?;
    if year == 0 || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=months[month - 1]).contains(&date) {
        return None;
    }
    let past = year - 1;
    Some(
        365 * past + past / 4 - past / 100
            + past / 400
            + months[..month - 1].iter().sum::<usize>()
            + date,
    )
}
