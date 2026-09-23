//! DiskTidy's read-only, headless eDirStat adapter.

use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use edirstat::{
    arena::{FileNode, NO_INDEX},
    coordinator::{Coordinator, SharedState},
    traversal::{NodeMeta, TraversalEngine},
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Engine {
    Auto,
    Mft,
    Walk,
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Read-only eDirStat scan into a compact SQLite snapshot"
)]
struct Args {
    /// Existing directory or drive root.
    path: PathBuf,
    /// New SQLite snapshot; an existing file is never overwritten.
    #[arg(long)]
    output: PathBuf,
    #[arg(long, value_enum, default_value_t = Engine::Auto)]
    engine: Engine,
    #[arg(long)]
    same_filesystem: bool,
    /// Stop without publishing a snapshot if enumeration exceeds this budget.
    #[arg(long)]
    max_files: Option<u64>,
    #[arg(long)]
    max_entries: Option<u64>,
    #[arg(long)]
    max_seconds: Option<f64>,
    #[arg(long)]
    exclude: Vec<PathBuf>,
    /// UTF-8 JSONL events are written to this new file instead of stdout.
    #[arg(long)]
    progress_file: Option<PathBuf>,
    /// A file created by the caller requests cooperative cancellation.
    #[arg(long)]
    cancel_file: Option<PathBuf>,
    #[arg(long, default_value = "standalone")]
    task_id: String,
}

type EventSink = Arc<Mutex<Box<dyn Write + Send>>>;

/// SQLite layout version shared with DiskTidy's `sqlite_snapshot.FORMAT_VERSION`.
const FORMAT_VERSION: &str = "2";
/// Offset between the Windows `FILETIME` epoch (1601) and the Unix epoch, in 100 ns units.
const FILETIME_UNIX_EPOCH: i128 = 116_444_736_000_000_000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

struct PendingSnapshot(PathBuf);

struct SnapshotExport<'a> {
    output: &'a Path,
    task_id: &'a str,
    root: &'a Path,
    excludes: &'a [PathBuf],
    backend: &'a str,
    metas: &'a [NodeMeta],
    sink: &'a EventSink,
    cancel: &'a AtomicBool,
}

impl Drop for PendingSnapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn emit(sink: &EventSink, event: &Value) -> Result<()> {
    let mut output = sink
        .lock()
        .map_err(|_| anyhow::anyhow!("progress writer lock poisoned"))?;
    serde_json::to_writer(&mut *output, event)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn new_sink(path: Option<&Path>) -> Result<EventSink> {
    let output: Box<dyn Write + Send> = match path {
        Some(path) => Box::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .with_context(|| format!("Cannot create progress file {}", path.display()))?,
        ),
        None => Box::new(io::stdout()),
    };
    Ok(Arc::new(Mutex::new(output)))
}

/// Converts a Windows `FILETIME` to Unix nanoseconds; `None` for 0 (unknown).
fn filetime_to_unix_ns(filetime: u64) -> Option<i64> {
    if filetime == 0 {
        return None;
    }
    i64::try_from((i128::from(filetime) - FILETIME_UNIX_EPOCH) * 100).ok()
}

/// Bytes allocated under each node: a file's own allocation, or the sum over a
/// directory's files. `None` when any contributing allocation is unknown.
fn subtree_allocations(nodes: &[FileNode], metas: &[NodeMeta]) -> Vec<Option<u64>> {
    let mut totals: Vec<Option<u64>> = nodes
        .iter()
        .zip(metas)
        .map(|(node, meta)| {
            if node.is_directory() {
                Some(0)
            } else if meta.allocated_size == u64::MAX {
                None
            } else {
                Some(meta.allocated_size)
            }
        })
        .collect();
    // Children always follow their parent in the arena.
    for idx in (1..totals.len()).rev() {
        let parent = nodes[idx].parent as usize;
        if nodes[idx].parent == NO_INDEX || parent >= idx {
            continue;
        }
        totals[parent] = match (totals[parent], totals[idx]) {
            (Some(total), Some(child)) => Some(total.saturating_add(child)),
            _ => None,
        };
    }
    totals
}

/// Reparse tag column: 0 without a reparse point, `None` when the tag was not read.
fn reparse_tag_value(meta: &NodeMeta) -> Option<i64> {
    if meta.reparse_tag != 0 {
        Some(i64::from(meta.reparse_tag))
    } else if meta.attributes != u32::MAX && meta.attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        Some(0)
    } else {
        None
    }
}

fn write_snapshot(
    snapshot: &edirstat::arena::FileArenaSnapshot,
    export: &SnapshotExport<'_>,
) -> Result<(u64, u64, u64)> {
    let output = export.output;
    let task_id = export.task_id;
    let root = export.root;
    let excludes = export.excludes;
    let backend = export.backend;
    let metas = export.metas;
    let sink = export.sink;
    let cancel = export.cancel;
    if output.exists() {
        bail!("Output already exists: {}", output.display());
    }
    let file_name = output.file_name().context("Output must have a file name")?;
    let pending = output.with_file_name(format!(
        "{}.{}.pending",
        file_name.to_string_lossy(),
        task_id
    ));
    if pending.exists() {
        bail!("Pending output already exists: {}", pending.display());
    }
    let _pending_cleanup = PendingSnapshot(pending.clone());
    let mut db = Connection::open(&pending)?;
    db.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA synchronous=NORMAL;
        CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE nodes (
            id INTEGER PRIMARY KEY, parent_id INTEGER, name TEXT NOT NULL,
            is_dir INTEGER NOT NULL, size INTEGER NOT NULL, allocated_size INTEGER,
            modified_ns INTEGER, created_ns INTEGER, flags INTEGER NOT NULL,
            file_count INTEGER NOT NULL, file_id INTEGER, attributes INTEGER,
            reparse_tag INTEGER, link_count INTEGER
        );
        CREATE TABLE directories (node_id INTEGER PRIMARY KEY, relative_path TEXT NOT NULL);",
    )?;
    let mut dir_paths: HashMap<u32, String> = HashMap::new();
    let mut excluded_dirs: HashSet<u32> = HashSet::new();
    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut bytes = 0u64;
    let mut allocated_bytes = 0u64;
    let mut unknown_allocations = 0u64;
    let allocations = subtree_allocations(&snapshot.nodes, metas);
    let root_text = root.to_string_lossy().to_string();
    let tx = db.transaction()?;
    {
        let mut node_insert = tx.prepare_cached(
            "INSERT INTO nodes
            (id,parent_id,name,is_dir,size,allocated_size,modified_ns,created_ns,flags,file_count,
            file_id,attributes,reparse_tag,link_count)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )?;
        let mut dir_insert =
            tx.prepare_cached("INSERT INTO directories(node_id,relative_path) VALUES (?,?)")?;
        for (idx, node) in snapshot.nodes.iter().enumerate() {
            if idx % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                bail!("Scan cancelled during SQLite export");
            }
            let id = u32::try_from(idx).context("Too many arena nodes")?;
            let parent = node.parent;
            if parent != NO_INDEX && excluded_dirs.contains(&parent) {
                if node.is_directory() {
                    excluded_dirs.insert(id);
                }
                continue;
            }
            let name = snapshot
                .string_pool
                .get(node.name_id)
                .context("Unresolved arena name")?;
            let relative = if parent == NO_INDEX {
                String::new()
            } else {
                let prefix = dir_paths
                    .get(&parent)
                    .context("Arena child without parent directory")?;
                if prefix.is_empty() {
                    name.to_owned()
                } else {
                    format!("{prefix}{}{name}", std::path::MAIN_SEPARATOR)
                }
            };
            if node.is_directory() && idx != 0 {
                let full_path = root.join(&relative);
                if excludes
                    .iter()
                    .any(|excluded| full_path.starts_with(excluded))
                {
                    excluded_dirs.insert(id);
                    continue;
                }
            }
            let meta = metas.get(idx).copied().unwrap_or(NodeMeta::UNKNOWN);
            // Directories keep the arena's propagated time; files prefer full precision.
            let precise_modified = if node.is_directory() {
                None
            } else {
                filetime_to_unix_ns(meta.modified_filetime)
            };
            let seconds_modified = if node.modified_timestamp == 0 {
                None
            } else {
                Some(i64::from(node.modified_timestamp) * 1_000_000_000)
            };
            let modified_ns = precise_modified.or(seconds_modified);
            let allocated = allocations.get(idx).copied().flatten();
            let created_ns = if node.created_timestamp == 0 {
                None
            } else {
                Some(i64::from(node.created_timestamp) * 1_000_000_000)
            };
            node_insert.execute(params![
                i64::from(id),
                if parent == NO_INDEX {
                    None
                } else {
                    Some(i64::from(parent))
                },
                if idx == 0 { root_text.as_str() } else { name },
                i64::from(node.is_directory()),
                i64::try_from(node.size)?,
                allocated.map(i64::try_from).transpose()?,
                modified_ns,
                created_ns,
                i64::from(node.flags),
                i64::from(node.file_count),
                // Bit-cast: NTFS file references use all 64 bits.
                (meta.file_id != 0).then_some(meta.file_id as i64),
                (meta.attributes != u32::MAX).then_some(i64::from(meta.attributes)),
                reparse_tag_value(&meta),
                (meta.link_count != 0).then_some(i64::from(meta.link_count))
            ])?;
            if node.is_directory() {
                dir_insert.execute(params![i64::from(id), &relative])?;
                dir_paths.insert(id, relative);
                dirs += 1;
            } else {
                files += 1;
                bytes = bytes.saturating_add(node.size);
                match allocated {
                    Some(value) => allocated_bytes = allocated_bytes.saturating_add(value),
                    None => unknown_allocations += 1,
                }
            }
        }
    }
    tx.execute(
        "INSERT INTO metadata VALUES ('format_version',?)",
        [FORMAT_VERSION],
    )?;
    tx.execute("INSERT INTO metadata VALUES ('root',?)", [&root_text])?;
    tx.execute("INSERT INTO metadata VALUES ('backend',?)", [backend])?;
    tx.execute(
        "INSERT INTO metadata VALUES ('file_count',?)",
        [files.to_string()],
    )?;
    tx.execute(
        "INSERT INTO metadata VALUES ('directory_count',?)",
        [dirs.to_string()],
    )?;
    tx.execute(
        "INSERT INTO metadata VALUES ('logical_bytes',?)",
        [bytes.to_string()],
    )?;
    // Sum over files whose allocation is known; the count below says how many are not.
    tx.execute(
        "INSERT INTO metadata VALUES ('allocated_bytes',?)",
        [allocated_bytes.to_string()],
    )?;
    tx.execute(
        "INSERT INTO metadata VALUES ('unknown_allocation_files',?)",
        [unknown_allocations.to_string()],
    )?;
    tx.commit()?;
    emit(
        sink,
        &json!({"version":1,"type":"progress","phase":"index","files":files,"dirs":dirs}),
    )?;
    db.execute_batch(
        "CREATE INDEX idx_nodes_size ON nodes(is_dir,size DESC,id);
        CREATE INDEX idx_nodes_parent ON nodes(parent_id,id);
        CREATE INDEX idx_nodes_name ON nodes(name);
        CREATE INDEX idx_directories_path ON directories(relative_path);",
    )?;
    if cancel.load(Ordering::Relaxed) {
        bail!("Scan cancelled during SQLite export");
    }
    db.close().map_err(|(_, error)| error)?;
    std::fs::rename(&pending, output)?;
    Ok((files, dirs, bytes))
}

fn run(args: &Args, sink: &EventSink) -> Result<()> {
    if args
        .max_seconds
        .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0)
    {
        bail!("max-seconds must be positive and finite");
    }
    if args.task_id.is_empty()
        || !args
            .task_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("task-id must contain only ASCII letters, digits, or hyphens");
    }
    let root = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot access scan root {}", args.path.display()))?;
    if !root.is_dir() {
        bail!("Scan root must be a directory");
    }
    let excludes = args
        .exclude
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect::<Vec<_>>();
    let started = Instant::now();
    emit(
        sink,
        &json!({"version":1,"type":"start","task_id":args.task_id,
        "root":args.path.to_string_lossy(),"engine":format!("{:?}", args.engine).to_ascii_lowercase()}),
    )?;

    let shared = Arc::new(SharedState::new());
    let mut traversal = TraversalEngine::new(shared.scan_stats.clone());
    traversal = match args.engine {
        Engine::Auto => traversal,
        Engine::Mft => traversal.mft_only(),
        Engine::Walk => traversal.without_mft(),
    };
    let (sender, receiver) = crossbeam::channel::unbounded();
    let handle = traversal.start_traversal(
        root.clone(),
        args.same_filesystem,
        shared.scan_cancel.clone(),
        sender,
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    let scan_finished = Arc::new(AtomicBool::new(false));
    let budget_exceeded = Arc::new(AtomicBool::new(false));
    let ticker = {
        let shared = shared.clone();
        let sink = sink.clone();
        let stop = stop.clone();
        let scan_finished = scan_finished.clone();
        let budget_exceeded = budget_exceeded.clone();
        let cancel_path = args.cancel_file.clone();
        let max_files = args.max_files;
        let max_entries = args.max_entries;
        let max_seconds = args.max_seconds;
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if cancel_path.as_ref().is_some_and(|path| path.exists()) {
                    shared.scan_cancel.store(true, Ordering::SeqCst);
                }
                let stats = &shared.scan_stats;
                let files = stats.files_scanned.load(Ordering::Relaxed) as u64;
                let dirs = stats.dirs_scanned.load(Ordering::Relaxed) as u64;
                if !scan_finished.load(Ordering::SeqCst)
                    && (max_files.is_some_and(|limit| files >= limit)
                        || max_entries.is_some_and(|limit| files.saturating_add(dirs) >= limit)
                        || max_seconds
                            .is_some_and(|limit| started.elapsed().as_secs_f64() >= limit))
                {
                    budget_exceeded.store(true, Ordering::SeqCst);
                    shared.scan_cancel.store(true, Ordering::SeqCst);
                }
                let _ = emit(
                    &sink,
                    &json!({"version":1,"type":"progress","phase":"scan",
                    "files":files,
                    "dirs":dirs,
                    "bytes":stats.bytes_scanned.load(Ordering::Relaxed)}),
                );
                thread::sleep(Duration::from_millis(500));
            }
        })
    };
    let mut coordinator = Coordinator::new(receiver, shared.clone());
    let metas = coordinator.run_coordinator_loop_headless_with_metadata(&root.to_string_lossy());
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("Traversal thread panicked"))?;
    scan_finished.store(true, Ordering::SeqCst);
    let scanned_files = shared.scan_stats.files_scanned.load(Ordering::Relaxed) as u64;
    let scanned_dirs = shared.scan_stats.dirs_scanned.load(Ordering::Relaxed) as u64;
    if budget_exceeded.load(Ordering::SeqCst)
        || args.max_files.is_some_and(|limit| scanned_files >= limit)
        || args
            .max_entries
            .is_some_and(|limit| scanned_files.saturating_add(scanned_dirs) >= limit)
        || args
            .max_seconds
            .is_some_and(|limit| started.elapsed().as_secs_f64() >= limit)
    {
        bail!("Scan enumeration budget exceeded");
    }
    if matches!(args.engine, Engine::Mft) && shared.scan_stats.backend.load(Ordering::SeqCst) != 1 {
        bail!("Raw MFT scan unavailable for this root");
    }
    if shared.scan_cancel.load(Ordering::SeqCst) {
        emit(
            sink,
            &json!({"version":1,"type":"cancelled","task_id":args.task_id}),
        )?;
        bail!("Scan cancelled");
    }
    if shared.scan_stats.mft_fallback.load(Ordering::SeqCst) {
        emit(
            sink,
            &json!({"version":1,"type":"fallback","from":"mft","to":"walk",
            "reason":"Raw MFT scan unavailable; directory traversal used"}),
        )?;
    }
    let backend = match shared.scan_stats.backend.load(Ordering::SeqCst) {
        1 => "mft",
        2 => "walk",
        _ => "unknown",
    };
    let snapshot = shared.current_snapshot.load();
    if snapshot.nodes.is_empty() {
        bail!("Scan returned an empty arena");
    }
    if metas.len() != snapshot.nodes.len() {
        bail!("Scan metadata does not match the arena");
    }
    emit(
        sink,
        &json!({"version":1,"type":"progress","phase":"write","nodes":snapshot.nodes.len()}),
    )?;
    let export = SnapshotExport {
        output: &args.output,
        task_id: &args.task_id,
        root: &root,
        excludes: &excludes,
        backend,
        metas: &metas,
        sink,
        cancel: shared.scan_cancel.as_ref(),
    };
    let (files, dirs, bytes) = write_snapshot(&snapshot, &export)?;
    stop.store(true, Ordering::SeqCst);
    let _ = ticker.join();
    emit(
        sink,
        &json!({"version":1,"type":"complete","task_id":args.task_id,
        "backend":backend,"files":files,"dirs":dirs,"bytes":bytes,
        "elapsed_ms":started.elapsed().as_millis()}),
    )?;
    Ok(())
}

fn main() {
    let args = Args::parse();
    let sink = match new_sink(args.progress_file.as_deref()) {
        Ok(sink) => sink,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    };
    if let Err(error) = run(&args, &sink) {
        let _ = emit(
            &sink,
            &json!({"version":1,"type":"error","task_id":args.task_id,
            "message":format!("{error:#}")}),
        );
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
