//! DiskTidy's read-only, headless eDirStat adapter.

use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use edirstat::{arena::NO_INDEX, coordinator::{Coordinator, SharedState}, traversal::TraversalEngine};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Engine {
    Auto,
    Walk,
}

#[derive(Parser, Debug)]
#[command(version, about = "Read-only eDirStat scan into a compact SQLite snapshot")]
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

fn emit(sink: &EventSink, event: &Value) -> Result<()> {
    let mut output = sink.lock().map_err(|_| anyhow::anyhow!("progress writer lock poisoned"))?;
    serde_json::to_writer(&mut *output, event)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn new_sink(path: Option<&Path>) -> Result<EventSink> {
    let output: Box<dyn Write + Send> = match path {
        Some(path) => Box::new(OpenOptions::new().write(true).create_new(true).open(path)
            .with_context(|| format!("Cannot create progress file {}", path.display()))?),
        None => Box::new(io::stdout()),
    };
    Ok(Arc::new(Mutex::new(output)))
}

fn write_snapshot(
    output: &Path,
    task_id: &str,
    root: &Path,
    snapshot: &edirstat::arena::FileArenaSnapshot,
    excludes: &[PathBuf],
    backend: &str,
    sink: &EventSink,
    cancel: &AtomicBool,
) -> Result<(u64, u64, u64)> {
    if output.exists() {
        bail!("Output already exists: {}", output.display());
    }
    let file_name = output.file_name().context("Output must have a file name")?;
    let pending = output.with_file_name(format!("{}.{}.pending", file_name.to_string_lossy(), task_id));
    if pending.exists() {
        bail!("Pending output already exists: {}", pending.display());
    }
    let mut db = Connection::open(&pending)?;
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=NORMAL;
        CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE nodes (
            id INTEGER PRIMARY KEY, parent_id INTEGER, name TEXT NOT NULL,
            is_dir INTEGER NOT NULL, size INTEGER NOT NULL, allocated_size INTEGER,
            modified_ns INTEGER, created_ns INTEGER, flags INTEGER NOT NULL,
            file_count INTEGER NOT NULL
        );
        CREATE TABLE directories (node_id INTEGER PRIMARY KEY, relative_path TEXT NOT NULL);")?;
    let mut dir_paths: HashMap<u32, String> = HashMap::new();
    let mut excluded_dirs: HashSet<u32> = HashSet::new();
    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut bytes = 0u64;
    let root_text = root.to_string_lossy().to_string();
    let tx = db.transaction()?;
    {
        let mut node_insert = tx.prepare_cached("INSERT INTO nodes
            (id,parent_id,name,is_dir,size,allocated_size,modified_ns,created_ns,flags,file_count)
            VALUES (?,?,?,?,?,?,?,?,?,?)")?;
        let mut dir_insert = tx.prepare_cached("INSERT INTO directories(node_id,relative_path) VALUES (?,?)")?;
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
            let name = snapshot.string_pool.get(node.name_id).context("Unresolved arena name")?;
            let relative = if parent == NO_INDEX {
                String::new()
            } else {
                let prefix = dir_paths.get(&parent).context("Arena child without parent directory")?;
                if prefix.is_empty() { name.to_owned() }
                else { format!("{prefix}{}{name}", std::path::MAIN_SEPARATOR) }
            };
            if node.is_directory() && idx != 0 {
                let full_path = root.join(&relative);
                if excludes.iter().any(|excluded| full_path.starts_with(excluded)) {
                    excluded_dirs.insert(id);
                    continue;
                }
            }
            let modified_ns = if node.modified_timestamp == 0 { None }
                else { Some(i64::from(node.modified_timestamp) * 1_000_000_000) };
            let created_ns = if node.created_timestamp == 0 { None }
                else { Some(i64::from(node.created_timestamp) * 1_000_000_000) };
            node_insert.execute(params![
                i64::from(id), if parent == NO_INDEX { None } else { Some(i64::from(parent)) },
                if idx == 0 { root_text.as_str() } else { name },
                i64::from(node.is_directory()), i64::try_from(node.size)?,
                Option::<i64>::None, modified_ns, created_ns, i64::from(node.flags),
                i64::from(node.file_count)
            ])?;
            if node.is_directory() {
                dir_insert.execute(params![i64::from(id), &relative])?;
                dir_paths.insert(id, relative);
                dirs += 1;
            } else {
                files += 1;
                bytes = bytes.saturating_add(node.size);
            }
        }
    }
    tx.execute("INSERT INTO metadata VALUES ('format_version','1')", [])?;
    tx.execute("INSERT INTO metadata VALUES ('root',?)", [&root_text])?;
    tx.execute("INSERT INTO metadata VALUES ('backend',?)", [backend])?;
    tx.execute("INSERT INTO metadata VALUES ('file_count',?)", [files.to_string()])?;
    tx.execute("INSERT INTO metadata VALUES ('directory_count',?)", [dirs.to_string()])?;
    tx.execute("INSERT INTO metadata VALUES ('logical_bytes',?)", [bytes.to_string()])?;
    tx.commit()?;
    emit(sink, &json!({"version":1,"type":"progress","phase":"index","files":files,"dirs":dirs}))?;
    db.execute_batch("CREATE INDEX idx_nodes_size ON nodes(is_dir,size DESC,id);
        CREATE INDEX idx_nodes_parent ON nodes(parent_id,id);
        CREATE INDEX idx_nodes_name ON nodes(name);
        CREATE INDEX idx_directories_path ON directories(relative_path);")?;
    if cancel.load(Ordering::Relaxed) {
        bail!("Scan cancelled during SQLite export");
    }
    db.close().map_err(|(_, error)| error)?;
    std::fs::rename(&pending, output)?;
    Ok((files, dirs, bytes))
}

fn run(args: &Args, sink: &EventSink) -> Result<()> {
    if args.task_id.is_empty() || !args.task_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        bail!("task-id must contain only ASCII letters, digits, or hyphens");
    }
    let root = std::fs::canonicalize(&args.path)
        .with_context(|| format!("Cannot access scan root {}", args.path.display()))?;
    if !root.is_dir() {
        bail!("Scan root must be a directory");
    }
    let excludes = args.exclude.iter().filter_map(|path| std::fs::canonicalize(path).ok()).collect::<Vec<_>>();
    let started = Instant::now();
    emit(sink, &json!({"version":1,"type":"start","task_id":args.task_id,
        "root":args.path.to_string_lossy(),"engine":format!("{:?}", args.engine).to_ascii_lowercase()}))?;

    let shared = Arc::new(SharedState::new());
    let mut traversal = TraversalEngine::new(shared.scan_stats.clone());
    if matches!(args.engine, Engine::Walk) {
        traversal = traversal.without_mft();
    }
    let (sender, receiver) = crossbeam::channel::unbounded();
    let handle = traversal.start_traversal(root.clone(), args.same_filesystem,
        shared.scan_cancel.clone(), sender)?;
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let shared = shared.clone();
        let sink = sink.clone();
        let stop = stop.clone();
        let cancel_path = args.cancel_file.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if cancel_path.as_ref().is_some_and(|path| path.exists()) {
                    shared.scan_cancel.store(true, Ordering::SeqCst);
                }
                let stats = &shared.scan_stats;
                let _ = emit(&sink, &json!({"version":1,"type":"progress","phase":"scan",
                    "files":stats.files_scanned.load(Ordering::Relaxed),
                    "dirs":stats.dirs_scanned.load(Ordering::Relaxed),
                    "bytes":stats.bytes_scanned.load(Ordering::Relaxed)}));
                thread::sleep(Duration::from_millis(500));
            }
        })
    };
    let mut coordinator = Coordinator::new(receiver, shared.clone());
    coordinator.run_coordinator_loop_headless(&root.to_string_lossy());
    stop.store(true, Ordering::SeqCst);
    let _ = ticker.join();
    handle.join().map_err(|_| anyhow::anyhow!("Traversal thread panicked"))?;
    if shared.scan_cancel.load(Ordering::SeqCst) {
        emit(sink, &json!({"version":1,"type":"cancelled","task_id":args.task_id}))?;
        bail!("Scan cancelled");
    }
    if shared.scan_stats.mft_fallback.load(Ordering::SeqCst) {
        emit(sink, &json!({"version":1,"type":"fallback","from":"mft","to":"walk",
            "reason":"Raw MFT scan unavailable; directory traversal used"}))?;
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
    emit(sink, &json!({"version":1,"type":"progress","phase":"write","nodes":snapshot.nodes.len()}))?;
    let (files, dirs, bytes) = write_snapshot(&args.output, &args.task_id, &root,
        &snapshot, &excludes, backend, sink, &shared.scan_cancel)?;
    emit(sink, &json!({"version":1,"type":"complete","task_id":args.task_id,
        "backend":backend,"files":files,"dirs":dirs,"bytes":bytes,
        "elapsed_ms":started.elapsed().as_millis()}))?;
    Ok(())
}

fn main() {
    let args = Args::parse();
    let sink = match new_sink(args.progress_file.as_deref()) {
        Ok(sink) => sink,
        Err(error) => { eprintln!("{error:#}"); std::process::exit(1); }
    };
    if let Err(error) = run(&args, &sink) {
        let _ = emit(&sink, &json!({"version":1,"type":"error","task_id":args.task_id,
            "message":format!("{error:#}")}));
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
