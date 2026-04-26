#![allow(dead_code)]

use anyhow::{Context as _, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MEMORY_LOG_SCHEMA_VERSION: u32 = 1;
pub const MEMORY_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

static EXTRA_MANAGED_PIDS: OnceLock<Mutex<BTreeSet<u32>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct MemorySnapshot {
    pub ts_ms: u128,
    pub app_run_id: String,
    pub root_pid: u32,
    pub process_count: usize,
    pub rss_bytes_total: u64,
    pub rss_bytes_main: u64,
    pub rss_bytes_children: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemorySummary {
    pub run_id: Option<String>,
    pub sample_interval_ms: Option<u64>,
    pub sample_count: usize,
    pub duration_ms: u128,
    pub mean_rss_bytes: u64,
    pub min_rss_bytes: u64,
    pub max_rss_bytes: u64,
    pub p95_rss_bytes: u64,
}

#[derive(Clone, Debug)]
struct LoggerConfig {
    path: PathBuf,
    app_run_id: String,
    workspace_root: String,
    version: String,
    interval: Duration,
    root_pid: u32,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct ProcessNode {
    parent_pid: u32,
    rss_bytes: u64,
}

#[allow(dead_code)]
pub fn register_managed_pid(pid: u32) {
    let managed = EXTRA_MANAGED_PIDS.get_or_init(|| Mutex::new(BTreeSet::new()));
    if let Ok(mut pids) = managed.lock() {
        pids.insert(pid);
    }
}

pub fn start_memory_logger(workspace_root: &Path, app_run_id: &str) -> anyhow::Result<()> {
    let config = LoggerConfig {
        path: paths::memory_log_file().clone(),
        app_run_id: app_run_id.to_string(),
        workspace_root: workspace_root.display().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        interval: MEMORY_SAMPLE_INTERVAL,
        root_pid: std::process::id(),
        started_at: Instant::now(),
    };
    start_memory_logger_with_config(config)
}

pub fn analyze_current_memory_log() -> anyhow::Result<MemorySummary> {
    analyze_memory_log(paths::memory_log_file())
}

pub fn analyze_memory_log(path: &Path) -> anyhow::Result<MemorySummary> {
    let file = File::open(path)
        .with_context(|| format!("opening memory log for analysis: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut run_id = None;
    let mut sample_interval_ms = None;
    let mut samples = Vec::new();
    let mut first_ts_ms = None;
    let mut last_ts_ms = None;

    for line_result in reader.lines() {
        let line = line_result
            .with_context(|| format!("reading memory log line from {}", path.display()))?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(event) = value.get("event").and_then(Value::as_str) else {
            continue;
        };
        match event {
            "memory.run_started" => {
                if run_id.is_none() {
                    run_id = value
                        .get("app_run_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                }
                if sample_interval_ms.is_none() {
                    sample_interval_ms = value.get("sample_interval_ms").and_then(Value::as_u64);
                }
            }
            "memory.sample" => {
                let Some(rss_bytes_total) = value.get("rss_bytes_total").and_then(Value::as_u64)
                else {
                    continue;
                };
                if let Some(ts_ms) = value.get("ts_ms").and_then(Value::as_u64) {
                    let ts_ms = u128::from(ts_ms);
                    first_ts_ms.get_or_insert(ts_ms);
                    last_ts_ms = Some(ts_ms);
                }
                if run_id.is_none() {
                    run_id = value
                        .get("app_run_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                }
                samples.push(rss_bytes_total);
            }
            _ => {}
        }
    }

    if samples.is_empty() {
        bail!("no memory.sample records found in {}", path.display());
    }

    let mut sorted = samples.clone();
    sorted.sort_unstable();

    let sample_count = samples.len();
    let total: u128 = samples.iter().map(|value| u128::from(*value)).sum();
    let mean_rss_bytes = (total / sample_count as u128) as u64;
    let min_rss_bytes = sorted.first().copied().unwrap_or_default();
    let max_rss_bytes = sorted.last().copied().unwrap_or_default();
    let p95_index = nearest_rank_index(sample_count, 95);
    let p95_rss_bytes = sorted.get(p95_index).copied().unwrap_or(max_rss_bytes);
    let duration_ms = match (first_ts_ms, last_ts_ms) {
        (Some(first), Some(last)) => last.saturating_sub(first),
        _ => 0,
    };

    Ok(MemorySummary {
        run_id,
        sample_interval_ms,
        sample_count,
        duration_ms,
        mean_rss_bytes,
        min_rss_bytes,
        max_rss_bytes,
        p95_rss_bytes,
    })
}

pub fn format_memory_summary(path: &Path, summary: &MemorySummary) -> String {
    let run_id = summary.run_id.as_deref().unwrap_or("unknown");
    let interval = summary
        .sample_interval_ms
        .map(|value| format!("{value} ms"))
        .unwrap_or_else(|| "unknown".to_string());

    format!(
        "Memory log: {}\nRun id: {run_id}\nSampling interval: {interval}\nSamples: {}\nDuration: {:.1} s\nMean RSS: {:.1} MB\nMin RSS: {:.1} MB\nMax RSS: {:.1} MB\nP95 RSS: {:.1} MB",
        path.display(),
        summary.sample_count,
        millis_to_seconds(summary.duration_ms),
        bytes_to_mb(summary.mean_rss_bytes),
        bytes_to_mb(summary.min_rss_bytes),
        bytes_to_mb(summary.max_rss_bytes),
        bytes_to_mb(summary.p95_rss_bytes),
    )
}

fn start_memory_logger_with_config(config: LoggerConfig) -> anyhow::Result<()> {
    if let Some(parent) = config.path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating memory log directory {}", parent.display()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&config.path)
        .with_context(|| format!("opening memory log {}", config.path.display()))?;

    write_json_line(
        &mut file,
        &json!({
            "schema_version": MEMORY_LOG_SCHEMA_VERSION,
            "event": "memory.run_started",
            "ts_ms": timestamp_ms(),
            "app_run_id": config.app_run_id,
            "workspace_root": config.workspace_root,
            "quorp_pid": config.root_pid,
            "root_pid": config.root_pid,
            "sample_interval_ms": config.interval.as_millis() as u64,
            "platform": std::env::consts::OS,
            "version": config.version,
        }),
    )?;

    match sample_process_tree(config.root_pid, &config.app_run_id) {
        Ok(snapshot) => write_snapshot_line(&mut file, &snapshot, config.started_at.elapsed())?,
        Err(error) => write_error_line(&mut file, &config.app_run_id, &error)?,
    }

    let mut thread_file = file;
    thread::spawn(move || {
        let mut wrote_error = false;
        loop {
            thread::sleep(config.interval);
            match sample_process_tree(config.root_pid, &config.app_run_id) {
                Ok(snapshot) => {
                    if write_snapshot_line(&mut thread_file, &snapshot, config.started_at.elapsed())
                        .is_ok()
                    {
                        wrote_error = false;
                    }
                }
                Err(error) => {
                    if !wrote_error
                        && write_error_line(&mut thread_file, &config.app_run_id, &error).is_ok()
                    {
                        wrote_error = true;
                    }
                }
            }
        }
    });

    Ok(())
}

fn sample_process_tree(root_pid: u32, app_run_id: &str) -> anyhow::Result<MemorySnapshot> {
    let extra_pids = registered_managed_pids();
    let (nodes, tree_pids) = collect_process_tree_nodes(root_pid, &extra_pids)?;
    let main_node = nodes
        .get(&root_pid)
        .copied()
        .ok_or_else(|| anyhow!("root pid {root_pid} was not found during memory sampling"))?;
    let rss_bytes_total: u64 = tree_pids
        .iter()
        .filter_map(|pid| nodes.get(pid))
        .map(|node| node.rss_bytes)
        .sum();
    let rss_bytes_main = main_node.rss_bytes;
    let rss_bytes_children = rss_bytes_total.saturating_sub(rss_bytes_main);

    Ok(MemorySnapshot {
        ts_ms: timestamp_ms(),
        app_run_id: app_run_id.to_string(),
        root_pid,
        process_count: tree_pids.len(),
        rss_bytes_total,
        rss_bytes_main,
        rss_bytes_children,
    })
}

fn write_snapshot_line(
    file: &mut File,
    snapshot: &MemorySnapshot,
    uptime: Duration,
) -> anyhow::Result<()> {
    write_json_line(
        file,
        &json!({
            "schema_version": MEMORY_LOG_SCHEMA_VERSION,
            "event": "memory.sample",
            "ts_ms": snapshot.ts_ms,
            "app_run_id": snapshot.app_run_id,
            "root_pid": snapshot.root_pid,
            "uptime_ms": uptime.as_millis() as u64,
            "process_count": snapshot.process_count,
            "rss_bytes_total": snapshot.rss_bytes_total,
            "rss_bytes_main": snapshot.rss_bytes_main,
            "rss_bytes_children": snapshot.rss_bytes_children,
        }),
    )
}

fn write_error_line(
    file: &mut File,
    app_run_id: &str,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    write_json_line(
        file,
        &json!({
            "schema_version": MEMORY_LOG_SCHEMA_VERSION,
            "event": "memory.sampler_error",
            "ts_ms": timestamp_ms(),
            "app_run_id": app_run_id,
            "detail": error.to_string(),
        }),
    )
}

fn write_json_line(file: &mut File, value: &Value) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *file, value).context("serializing memory log event")?;
    file.write_all(b"\n")
        .context("writing memory log newline")?;
    file.flush().context("flushing memory log event")?;
    Ok(())
}

fn registered_managed_pids() -> BTreeSet<u32> {
    EXTRA_MANAGED_PIDS
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .map(|pids| pids.clone())
        .unwrap_or_default()
}

fn nearest_rank_index(len: usize, percentile: usize) -> usize {
    if len == 0 {
        return 0;
    }
    percentile
        .saturating_mul(len)
        .div_ceil(100)
        .saturating_sub(1)
        .min(len.saturating_sub(1))
}

fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn millis_to_seconds(duration_ms: u128) -> f64 {
    duration_ms as f64 / 1000.0
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn collect_process_tree_nodes(
    root_pid: u32,
    extra_pids: &BTreeSet<u32>,
) -> anyhow::Result<(BTreeMap<u32, ProcessNode>, BTreeSet<u32>)> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        bail!("could not determine Linux page size");
    }
    let page_size = page_size as u64;

    let mut nodes = BTreeMap::new();
    let mut children = BTreeMap::<u32, Vec<u32>>::new();

    for entry_result in std::fs::read_dir("/proc").context("reading /proc directory")? {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_name = entry.file_name();
        let Some(pid) = file_name
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Some(node) = read_linux_process_node(pid, page_size)? else {
            continue;
        };
        children.entry(node.parent_pid).or_default().push(pid);
        nodes.insert(pid, node);
    }

    let tree = collect_reachable_pids(root_pid, extra_pids, &children, &nodes);
    Ok((nodes, tree))
}

#[cfg(target_os = "linux")]
fn read_linux_process_node(pid: u32, page_size: u64) -> anyhow::Result<Option<ProcessNode>> {
    let stat_path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = match std::fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", stat_path.display()));
        }
    };
    let Some(close_paren) = stat.rfind(')') else {
        return Ok(None);
    };
    let Some(fields) = stat.get(close_paren + 2..) else {
        return Ok(None);
    };
    let mut parts = fields.split_whitespace();
    let _state = parts.next();
    let Some(parent_pid) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
        return Ok(None);
    };

    let statm_path = PathBuf::from(format!("/proc/{pid}/statm"));
    let rss_pages = match std::fs::read_to_string(&statm_path) {
        Ok(statm) => statm
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", statm_path.display()));
        }
    };

    Ok(Some(ProcessNode {
        parent_pid,
        rss_bytes: rss_pages.saturating_mul(page_size),
    }))
}

#[cfg(target_os = "macos")]
fn collect_process_tree_nodes(
    root_pid: u32,
    extra_pids: &BTreeSet<u32>,
) -> anyhow::Result<(BTreeMap<u32, ProcessNode>, BTreeSet<u32>)> {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        bail!("could not enumerate macOS processes");
    }

    let mut pids = vec![0i32; count as usize + 32];
    let buffer_bytes = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
    let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), buffer_bytes) };
    if listed <= 0 {
        bail!("macOS proc_listallpids returned no processes");
    }

    let mut nodes = BTreeMap::new();
    let mut children = BTreeMap::<u32, Vec<u32>>::new();
    for pid in pids.into_iter().take(listed as usize) {
        if pid <= 0 {
            continue;
        }
        let Some(node) = read_macos_process_node(pid as u32) else {
            continue;
        };
        children
            .entry(node.parent_pid)
            .or_default()
            .push(pid as u32);
        nodes.insert(pid as u32, node);
    }

    let tree = collect_reachable_pids(root_pid, extra_pids, &children, &nodes);
    Ok((nodes, tree))
}

#[cfg(target_os = "macos")]
fn read_macos_process_node(pid: u32) -> Option<ProcessNode> {
    let pid = pid as i32;
    let mut bsd_info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let bsd_size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let bsd_result = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            bsd_info.as_mut_ptr().cast(),
            bsd_size,
        )
    };
    if bsd_result != bsd_size {
        return None;
    }

    let mut task_info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let task_size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let task_result = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            task_info.as_mut_ptr().cast(),
            task_size,
        )
    };
    if task_result != task_size {
        return None;
    }

    let bsd_info = unsafe { bsd_info.assume_init() };
    let task_info = unsafe { task_info.assume_init() };
    Some(ProcessNode {
        parent_pid: bsd_info.pbi_ppid,
        rss_bytes: task_info.pti_resident_size,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn collect_process_tree_nodes(
    _root_pid: u32,
    _extra_pids: &BTreeSet<u32>,
) -> anyhow::Result<(BTreeMap<u32, ProcessNode>, BTreeSet<u32>)> {
    bail!("memory fingerprint sampling is only supported on Linux and macOS")
}

fn collect_reachable_pids(
    root_pid: u32,
    extra_pids: &BTreeSet<u32>,
    children: &BTreeMap<u32, Vec<u32>>,
    nodes: &BTreeMap<u32, ProcessNode>,
) -> BTreeSet<u32> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root_pid];
    stack.extend(extra_pids.iter().copied());

    while let Some(pid) = stack.pop() {
        if !nodes.contains_key(&pid) || !seen.insert(pid) {
            continue;
        }
        if let Some(child_pids) = children.get(&pid) {
            stack.extend(child_pids.iter().copied());
        }
    }

    seen
}

#[cfg(test)]
fn clear_managed_pids_for_test() {
    if let Some(extra_pids) = EXTRA_MANAGED_PIDS.get()
        && let Ok(mut pids) = extra_pids.lock()
    {
        pids.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_p95_uses_expected_index() {
        assert_eq!(nearest_rank_index(1, 95), 0);
        assert_eq!(nearest_rank_index(10, 95), 9);
        assert_eq!(nearest_rank_index(20, 95), 18);
    }

    #[test]
    fn analyzer_ignores_non_sample_rows_and_reports_summary() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let log_path = temp_dir.path().join("memory.log");
        std::fs::write(
            &log_path,
            concat!(
                "{\"event\":\"memory.run_started\",\"app_run_id\":\"run-1\",\"sample_interval_ms\":1000}\n",
                "not-json\n",
                "{\"event\":\"memory.sample\",\"ts_ms\":1000,\"rss_bytes_total\":104857600}\n",
                "{\"event\":\"memory.sampler_error\",\"detail\":\"ignored\"}\n",
                "{\"event\":\"memory.sample\",\"ts_ms\":2000,\"rss_bytes_total\":209715200}\n"
            ),
        )
        .expect("write log");

        let summary = analyze_memory_log(&log_path).expect("analyze");
        assert_eq!(summary.run_id.as_deref(), Some("run-1"));
        assert_eq!(summary.sample_interval_ms, Some(1000));
        assert_eq!(summary.sample_count, 2);
        assert_eq!(summary.duration_ms, 1000);
        assert_eq!(summary.min_rss_bytes, 104_857_600);
        assert_eq!(summary.max_rss_bytes, 209_715_200);
        assert_eq!(summary.p95_rss_bytes, 209_715_200);
        assert_eq!(summary.mean_rss_bytes, 157_286_400);
    }

    #[test]
    fn analyzer_error_mentions_missing_path() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let missing = temp_dir.path().join("missing-memory.log");

        let error = analyze_memory_log(&missing).expect_err("missing log should fail");
        assert!(error.to_string().contains(&missing.display().to_string()));
    }

    #[test]
    fn formatting_memory_summary_is_human_readable() {
        let summary = MemorySummary {
            run_id: Some("run-2".to_string()),
            sample_interval_ms: Some(1000),
            sample_count: 3,
            duration_ms: 2000,
            mean_rss_bytes: 157_286_400,
            min_rss_bytes: 104_857_600,
            max_rss_bytes: 209_715_200,
            p95_rss_bytes: 209_715_200,
        };
        let rendered = format_memory_summary(Path::new("/tmp/QuorpMemory.log"), &summary);
        assert!(rendered.contains("Run id: run-2"));
        assert!(rendered.contains("Mean RSS: 150.0 MB"));
        assert!(rendered.contains("P95 RSS: 200.0 MB"));
    }

    #[test]
    fn start_memory_logger_truncates_and_writes_header_and_samples() {
        clear_managed_pids_for_test();
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let log_path = temp_dir.path().join("memory.log");
        std::fs::write(&log_path, "stale-data\n").expect("seed stale log");

        let config = LoggerConfig {
            path: log_path.clone(),
            app_run_id: "run-logger".to_string(),
            workspace_root: temp_dir.path().display().to_string(),
            version: "test-version".to_string(),
            interval: Duration::from_millis(20),
            root_pid: std::process::id(),
            started_at: Instant::now(),
        };

        start_memory_logger_with_config(config).expect("start logger");
        std::thread::sleep(Duration::from_millis(70));

        let contents = std::fs::read_to_string(&log_path).expect("read log");
        assert!(!contents.contains("stale-data"));
        assert!(contents.contains("\"event\":\"memory.run_started\""));
        assert!(contents.contains("\"event\":\"memory.sample\""));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn process_tree_sample_counts_child_processes() {
        #[allow(clippy::disallowed_methods)]
        fn spawn_sleep_child() -> std::io::Result<std::process::Child> {
            std::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 2")
                .spawn()
        }

        clear_managed_pids_for_test();
        let mut child = spawn_sleep_child().expect("spawn child");
        std::thread::sleep(Duration::from_millis(100));

        let snapshot = sample_process_tree(std::process::id(), "test-run").expect("sample");
        assert!(snapshot.process_count >= 2);
        assert!(snapshot.rss_bytes_total >= snapshot.rss_bytes_main);
        assert_eq!(
            snapshot.rss_bytes_total,
            snapshot.rss_bytes_main + snapshot.rss_bytes_children
        );

        if let Err(error) = child.kill()
            && error.kind() != std::io::ErrorKind::InvalidInput
        {
            panic!("kill child: {error}");
        }
        child.wait().expect("wait child");
    }
}
