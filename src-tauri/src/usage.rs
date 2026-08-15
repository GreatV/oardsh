use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::paths;

/// A scan reads every session transcript; a range switch does not change them.
const USAGE_TTL: Duration = Duration::from_secs(60);
/// One year of daily buckets, which is what the activity heatmap draws.
const WINDOW_DAYS: i64 = 364;
const FILE_LIMIT: usize = 5000;
/// A transcript this large is a runaway log, not a conversation worth counting.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MS_PER_DAY: i64 = 86_400_000;

/// Keyed by UTC offset: another timezone must not read these buckets.
static USAGE_CACHE: Mutex<Option<(Instant, i64, UsageReport)>> = Mutex::new(None);

#[derive(Debug, Default, Clone, Copy)]
struct Tokens {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

impl Tokens {
    fn add(&mut self, other: Tokens) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
    }

    /// Reasoning tokens are already inside the output count, so they are
    /// reported separately but never added in.
    fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

#[derive(Debug, Default)]
struct DayAccumulator {
    tokens: Tokens,
    messages: u64,
    models: HashMap<String, (Tokens, u64)>,
    sessions: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    model: String,
    total_tokens: u64,
    messages: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayUsage {
    /// Local calendar day, `YYYY-MM-DD`.
    day: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
    messages: u64,
    /// Session ordinals active on this day; the caller unions them across a
    /// range so one spanning midnight counts once.
    sessions: Vec<u32>,
    models: Vec<ModelUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    generated_at: u64,
    /// Contiguous local days, oldest first, ending on the caller's today.
    days: Vec<DayUsage>,
    files_scanned: u64,
}

/// Aggregate local dsh session transcripts into daily buckets.
///
/// `offset_minutes` is the caller's UTC offset (`-new Date().getTimezoneOffset()`);
/// days are bucketed in that local time so "today" matches what the user sees.
/// `force` skips the cache, so a manual refresh inside the TTL is not a no-op.
#[tauri::command]
pub async fn token_usage(
    offset_minutes: Option<i64>,
    force: Option<bool>,
) -> Result<UsageReport, String> {
    let offset = offset_minutes.unwrap_or(0).clamp(-14 * 60, 14 * 60);
    if !force.unwrap_or(false) {
        if let Some((computed, cached_offset, report)) = USAGE_CACHE
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .as_ref()
        {
            if *cached_offset == offset && computed.elapsed() < USAGE_TTL {
                return Ok(report.clone());
            }
        }
    }
    let report = blocking(move || scan_usage(offset)).await?;
    *USAGE_CACHE.lock().unwrap_or_else(|err| err.into_inner()) =
        Some((Instant::now(), offset, report.clone()));
    Ok(report)
}

fn scan_usage(offset_minutes: i64) -> Result<UsageReport, String> {
    let root = dsh_home()?.join("sessions");
    let mut files = Vec::new();
    collect_session_files(&root, &mut files, 0);
    files.sort();

    let today = day_index(now_millis(), offset_minutes);
    let first = today - WINDOW_DAYS;
    let mut buckets: HashMap<i64, DayAccumulator> = HashMap::new();
    let mut scanned = 0;

    for (index, path) in files.iter().enumerate() {
        let Ok(metadata) = fs::metadata(path) else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(text) = read_session_text(path) else {
            continue;
        };
        scanned += 1;
        collect_session(&text, index as u32, offset_minutes, first, &mut buckets);
    }

    let days = (first..=today)
        .map(|index| render_day(index, buckets.remove(&index).unwrap_or_default()))
        .collect();
    Ok(UsageReport {
        generated_at: now_millis() as u64 / 1000,
        days,
        files_scanned: scanned,
    })
}

fn render_day(index: i64, mut accumulator: DayAccumulator) -> DayUsage {
    let mut models: Vec<ModelUsage> = accumulator
        .models
        .drain()
        .map(|(model, (tokens, messages))| ModelUsage {
            model,
            total_tokens: tokens.total(),
            messages,
        })
        .collect();
    models.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.model.cmp(&b.model))
    });
    accumulator.sessions.sort_unstable();
    accumulator.sessions.dedup();
    DayUsage {
        day: format_day(index),
        input_tokens: accumulator.tokens.input,
        output_tokens: accumulator.tokens.output,
        cache_read_tokens: accumulator.tokens.cache_read,
        cache_write_tokens: accumulator.tokens.cache_write,
        reasoning_tokens: accumulator.tokens.reasoning,
        total_tokens: accumulator.tokens.total(),
        messages: accumulator.messages,
        sessions: accumulator.sessions,
        models,
    }
}

/// Walk one transcript in order, carrying the most recent request header's
/// model so each message is attributed to the model that produced it.
fn collect_session(
    text: &str,
    session: u32,
    offset_minutes: i64,
    first_day: i64,
    buckets: &mut HashMap<i64, DayAccumulator>,
) {
    let mut model = String::new();
    for line in text.lines() {
        // Chunk events dominate the file; reject them before the JSON parse.
        if !line.contains("assistant/message")
            && !line.contains("request/context")
            && !line.contains("request/header")
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let data = value.get("data");
        match value.get("type").and_then(Value::as_str) {
            Some("request/context") => {
                if let Some(name) = data
                    .and_then(|data| data.get("model"))
                    .and_then(Value::as_str)
                {
                    model = name.to_string();
                }
            }
            Some("request/header") => {
                if let Some(name) = data
                    .and_then(|data| data.pointer("/header/config/model"))
                    .and_then(Value::as_str)
                {
                    model = name.to_string();
                }
            }
            Some("assistant/message") => {
                let Some(usage) = data.and_then(|data| data.get("usage")) else {
                    continue;
                };
                let Some(time) = value.get("time").and_then(Value::as_i64) else {
                    continue;
                };
                let index = day_index(time, offset_minutes);
                if index < first_day {
                    continue;
                }
                let tokens = Tokens {
                    input: number(usage.get("inputTokens")),
                    output: number(usage.get("outputTokens")),
                    cache_read: number(usage.get("cacheReadTokens")),
                    cache_write: number(usage.get("cacheWriteTokens")),
                    reasoning: number(usage.get("reasoningTokens")),
                };
                let bucket = buckets.entry(index).or_default();
                bucket.tokens.add(tokens);
                bucket.messages += 1;
                bucket.sessions.push(session);
                let name = if model.is_empty() {
                    "unknown"
                } else {
                    model.as_str()
                };
                let entry = bucket.models.entry(name.to_string()).or_default();
                entry.0.add(tokens);
                entry.1 += 1;
            }
            _ => {}
        }
    }
}

fn collect_session_files(root: &Path, output: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 || output.len() >= FILE_LIMIT {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, output, depth + 1);
        } else {
            let name = path.to_string_lossy();
            if name.ends_with(".jsonl") || name.ends_with(".jsonl.zstd") {
                output.push(path);
            }
        }
    }
}

/// Run the scan off the IPC worker pool; a year of transcripts would otherwise
/// hold a Tauri worker thread.
async fn blocking<T, F>(body: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(body)
        .await
        .map_err(|err| err.to_string())?
}

fn number(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

fn dsh_home() -> Result<PathBuf, String> {
    paths::resolve_dsh_home()
        .map(PathBuf::from)
        .ok_or_else(|| "Could not resolve DSH_HOME".into())
}

fn read_session_text(path: &Path) -> Result<String, String> {
    if path.to_string_lossy().ends_with(".jsonl.zstd") {
        let file = fs::File::open(path).map_err(|err| err.to_string())?;
        let bytes = zstd::stream::decode_all(file).map_err(|err| err.to_string())?;
        String::from_utf8(bytes).map_err(|err| err.to_string())
    } else {
        fs::read_to_string(path).map_err(|err| err.to_string())
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Days since the epoch, in the caller's local time.
fn day_index(millis: i64, offset_minutes: i64) -> i64 {
    millis
        .saturating_add(offset_minutes.saturating_mul(60_000))
        .div_euclid(MS_PER_DAY)
}

/// `YYYY-MM-DD` for a day index, via Howard Hinnant's civil-from-days.
fn format_day(index: i64) -> String {
    let shifted = index + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::{collect_session, day_index, format_day, DayAccumulator};
    use std::collections::HashMap;

    #[test]
    fn formats_days_across_epoch_and_leap_years() {
        assert_eq!(format_day(0), "1970-01-01");
        assert_eq!(format_day(-1), "1969-12-31");
        assert_eq!(format_day(19_417), "2023-03-01"); // day after a non-leap Feb
        assert_eq!(format_day(20_512), "2026-02-28");
        assert_eq!(format_day(20_513), "2026-03-01");
    }

    #[test]
    fn buckets_by_local_day_not_utc() {
        // 2026-08-15T23:30Z is already the 16th in UTC+8.
        let millis = 1_786_836_600_000;
        assert_eq!(format_day(day_index(millis, 0)), "2026-08-15");
        assert_eq!(format_day(day_index(millis, 8 * 60)), "2026-08-16");
    }

    #[test]
    fn attributes_final_usage_to_the_requesting_model() {
        let text = r#"{"type":"request/context","time":1,"data":{"provider":"deepseek-official","model":"deepseek-v4-flash"}}
{"type":"assistant/chunk","time":2,"data":{"chunk":{"type":"usage","usage":{"inputTokens":99,"outputTokens":99}}}}
{"type":"assistant/message","time":86400000,"data":{"usage":{"inputTokens":10,"outputTokens":4,"cacheReadTokens":3,"cacheWriteTokens":2,"reasoningTokens":1}}}"#;
        let mut buckets: HashMap<i64, DayAccumulator> = HashMap::new();
        collect_session(text, 7, 0, 0, &mut buckets);

        assert_eq!(buckets.len(), 1, "chunk usage must not open its own bucket");
        let day = &buckets[&1];
        assert_eq!(day.tokens.total(), 19);
        assert_eq!(day.tokens.reasoning, 1);
        assert_eq!(day.messages, 1);
        assert_eq!(day.sessions, vec![7]);
        assert_eq!(day.models["deepseek-v4-flash"].1, 1);
    }

    #[test]
    fn drops_messages_older_than_the_window() {
        let text = r#"{"type":"assistant/message","time":0,"data":{"usage":{"inputTokens":10}}}"#;
        let mut buckets: HashMap<i64, DayAccumulator> = HashMap::new();
        collect_session(text, 0, 0, 5, &mut buckets);
        assert!(buckets.is_empty());
    }
}
