//! Batch export.
//!
//! Runs a list of [`ExportItem`]s, writing each to a file inside
//! `output_dir`. There are two runners:
//!
//! * [`run_batch`] — sequential, used by the synchronous in-process
//!   API and the existing export-preset flow.
//! * [`run_batch_parallel`] — `rayon` thread-pool runner with shared
//!   cancellation flag and progress callback. Used by the async
//!   bridge job runner for the UI "Export Pack" affordance.
//!
//! Both runners isolate each item: a failure in one does not abort
//! the rest. The terminal status carries a per-item error list.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use kcreate_core::document::DocumentGraph;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::pdf::{export_pdf_from_document, PdfExportError, PdfExportOptions, RasterPixelCache};
use crate::svg::{export_svg_from_document, SvgDocumentExportError, SvgExportOptions};

/// One item in a batch export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum ExportItem {
    /// Render the document to SVG.
    Svg {
        filename: String,
        node_ids: Vec<Uuid>,
        options: SvgExportOptions,
    },
    /// Render the document to PDF.
    Pdf {
        filename: String,
        options: PdfExportOptions,
    },
}

impl ExportItem {
    /// Filename the item will be written to.
    #[must_use]
    pub fn filename(&self) -> &str {
        match self {
            Self::Svg { filename, .. } | Self::Pdf { filename, .. } => filename,
        }
    }
}

/// Lifecycle status of a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BatchStatus {
    Pending,
    Running {
        completed: usize,
        total: usize,
    },
    Done {
        succeeded: usize,
        failed: usize,
        errors: Vec<String>,
    },
    Cancelled {
        completed: usize,
        total: usize,
    },
}

/// Persistent record of a batch export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExportJob {
    pub id: Uuid,
    pub items: Vec<ExportItem>,
    pub output_dir: PathBuf,
    pub status: BatchStatus,
}

#[derive(Debug, Error)]
pub enum BatchExportError {
    #[error("svg: {0}")]
    Svg(#[from] SvgDocumentExportError),
    #[error("pdf: {0}")]
    Pdf(#[from] PdfExportError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Mid-run progress snapshot reported by [`run_batch_parallel`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct BatchProgress {
    pub completed: usize,
    pub total: usize,
    pub current_item: String,
}

/// Final outcome of a parallel batch run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct BatchResult {
    pub succeeded: Vec<PathBuf>,
    pub failed: Vec<(String, String)>,
    pub duration_ms: u64,
    pub cancelled: bool,
}

/// Run every [`ExportItem`] in `job.items`, writing outputs into
/// `job.output_dir`. Mutates `job.status` to track progress.
///
/// Returns when every item has either succeeded or recorded an error.
/// The status transitions:
///
/// 1. `Pending` → `Running { completed: 0, total: N }`
/// 2. Per item, `completed` increments.
/// 3. After the last item, `Done { succeeded, failed, errors }`.
pub fn run_batch(
    job: &mut BatchExportJob,
    document: &DocumentGraph,
    rasters: &RasterPixelCache,
) -> Result<(), BatchExportError> {
    std::fs::create_dir_all(&job.output_dir)?;
    job.status = BatchStatus::Running {
        completed: 0,
        total: job.items.len(),
    };
    let mut errors: Vec<String> = Vec::new();
    let mut succeeded = 0usize;
    for (idx, item) in job.items.iter().enumerate() {
        match run_one(item, document, rasters, &job.output_dir) {
            Ok(()) => succeeded += 1,
            Err(e) => errors.push(format!("item {idx}: {e}")),
        }
        job.status = BatchStatus::Running {
            completed: idx + 1,
            total: job.items.len(),
        };
    }
    let failed = errors.len();
    job.status = BatchStatus::Done {
        succeeded,
        failed,
        errors,
    };
    Ok(())
}

/// Parallel batch runner backed by the `rayon` global thread pool.
///
/// `cancel` is consulted between items — already-in-flight work is
/// allowed to finish so the output directory does not hold partial
/// files. `progress_fn` is invoked once per completed item with the
/// running tally.
///
/// On success the returned [`BatchResult`] aggregates per-item
/// outcomes. The function never panics on a single item's failure;
/// it records the error and continues.
pub fn run_batch_parallel(
    job: &BatchExportJob,
    document: &DocumentGraph,
    rasters: &RasterPixelCache,
    cancel: &AtomicBool,
    progress_fn: impl Fn(BatchProgress) + Sync + Send,
) -> Result<BatchResult, BatchExportError> {
    std::fs::create_dir_all(&job.output_dir)?;
    let total = job.items.len();
    let completed = AtomicUsize::new(0);
    let start = Instant::now();
    let succeeded_lock = parking_lot::Mutex::new(Vec::<PathBuf>::with_capacity(total));
    let failed_lock = parking_lot::Mutex::new(Vec::<(String, String)>::new());

    job.items.par_iter().enumerate().for_each(|(idx, item)| {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let result = run_one(item, document, rasters, &job.output_dir);
        let name = item.filename().to_string();
        match result {
            Ok(()) => {
                succeeded_lock.lock().push(job.output_dir.join(&name));
            }
            Err(e) => {
                failed_lock.lock().push((name.clone(), format!("item {idx}: {e}")));
            }
        }
        let new_completed = completed.fetch_add(1, Ordering::SeqCst) + 1;
        progress_fn(BatchProgress {
            completed: new_completed,
            total,
            current_item: name,
        });
    });

    let cancelled = cancel.load(Ordering::SeqCst);
    Ok(BatchResult {
        succeeded: succeeded_lock.into_inner(),
        failed: failed_lock.into_inner(),
        duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        cancelled,
    })
}

fn run_one(
    item: &ExportItem,
    document: &DocumentGraph,
    rasters: &RasterPixelCache,
    output_dir: &Path,
) -> Result<(), BatchExportError> {
    match item {
        ExportItem::Svg {
            filename,
            node_ids,
            options,
        } => {
            let svg = export_svg_from_document(document, node_ids, options)?;
            let out = output_dir.join(filename);
            std::fs::write(&out, svg)?;
            Ok(())
        }
        ExportItem::Pdf { filename, options } => {
            let out = output_dir.join(filename);
            export_pdf_from_document(document, options, rasters, &out)?;
            Ok(())
        }
    }
}

/// Lightweight handle used by [`run_batch_parallel`] callers (the
/// async-job bridge runner) to share a cancellation flag with the
/// worker thread.
#[derive(Debug, Clone, Default)]
pub struct BatchCancel(pub Arc<AtomicBool>);

impl BatchCancel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn as_inner(&self) -> &AtomicBool {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{Bounds, Node, NodeType};
    use kcreate_vector::{PathPoint, PathSegment, VectorPath};
    use std::sync::Mutex;

    fn doc_with_one_rect() -> DocumentGraph {
        let mut doc = DocumentGraph::new();
        let page = doc.insert_node(Node::new(NodeType::Page, "Page")).unwrap();
        let path = VectorPath::new(vec![
            PathSegment::MoveTo(PathPoint::new(0.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(50.0, 0.0)),
            PathSegment::LineTo(PathPoint::new(50.0, 50.0)),
            PathSegment::LineTo(PathPoint::new(0.0, 50.0)),
            PathSegment::Close,
        ]);
        let mut n = Node::new(NodeType::VectorLayer, "rect");
        n.parent_id = Some(page);
        n.bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 50.0,
            height: 50.0,
        };
        n.metadata.insert(
            crate::svg::VECTOR_PATH_METADATA_KEY.to_string(),
            serde_json::to_value(&path).unwrap(),
        );
        doc.insert_node(n).unwrap();
        doc
    }

    #[test]
    fn batch_export_writes_each_item() {
        let doc = doc_with_one_rect();
        let tmpdir = tempfile::tempdir().unwrap();
        let mut job = BatchExportJob {
            id: Uuid::new_v4(),
            items: vec![
                ExportItem::Svg {
                    filename: "out.svg".into(),
                    node_ids: Vec::new(),
                    options: SvgExportOptions::default(),
                },
                ExportItem::Pdf {
                    filename: "out.pdf".into(),
                    options: PdfExportOptions::default(),
                },
            ],
            output_dir: tmpdir.path().to_path_buf(),
            status: BatchStatus::Pending,
        };
        run_batch(&mut job, &doc, &RasterPixelCache::new()).unwrap();
        assert!(matches!(
            job.status,
            BatchStatus::Done {
                succeeded: 2,
                failed: 0,
                ..
            }
        ));
        assert!(tmpdir.path().join("out.svg").exists());
        assert!(tmpdir.path().join("out.pdf").exists());
    }

    #[test]
    fn parallel_batch_writes_each_item_and_calls_progress() {
        let doc = doc_with_one_rect();
        let tmpdir = tempfile::tempdir().unwrap();
        let job = BatchExportJob {
            id: Uuid::new_v4(),
            items: vec![
                ExportItem::Svg {
                    filename: "a.svg".into(),
                    node_ids: Vec::new(),
                    options: SvgExportOptions::default(),
                },
                ExportItem::Svg {
                    filename: "b.svg".into(),
                    node_ids: Vec::new(),
                    options: SvgExportOptions::default(),
                },
                ExportItem::Svg {
                    filename: "c.svg".into(),
                    node_ids: Vec::new(),
                    options: SvgExportOptions::default(),
                },
            ],
            output_dir: tmpdir.path().to_path_buf(),
            status: BatchStatus::Pending,
        };
        let cancel = AtomicBool::new(false);
        let progress = Mutex::new(Vec::<BatchProgress>::new());
        let result = run_batch_parallel(&job, &doc, &RasterPixelCache::new(), &cancel, |p| {
            progress.lock().unwrap().push(p);
        })
        .expect("parallel batch");
        assert_eq!(result.succeeded.len(), 3);
        assert!(result.failed.is_empty());
        assert!(!result.cancelled);
        let prog = progress.into_inner().unwrap();
        assert_eq!(prog.len(), 3);
        // Every progress sample reports total = 3.
        for p in &prog {
            assert_eq!(p.total, 3);
        }
        // Each filename appears exactly once across the progress
        // samples, in some order.
        let mut names: Vec<String> = prog.iter().map(|p| p.current_item.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["a.svg".to_string(), "b.svg".to_string(), "c.svg".to_string()]
        );
    }

    #[test]
    fn parallel_batch_honours_cancel_flag() {
        let doc = doc_with_one_rect();
        let tmpdir = tempfile::tempdir().unwrap();
        let items = (0..16)
            .map(|i| ExportItem::Svg {
                filename: format!("x{i}.svg"),
                node_ids: Vec::new(),
                options: SvgExportOptions::default(),
            })
            .collect();
        let job = BatchExportJob {
            id: Uuid::new_v4(),
            items,
            output_dir: tmpdir.path().to_path_buf(),
            status: BatchStatus::Pending,
        };
        let cancel = AtomicBool::new(true); // Pre-cancelled.
        let result = run_batch_parallel(&job, &doc, &RasterPixelCache::new(), &cancel, |_| {})
            .expect("parallel batch");
        assert!(result.cancelled);
        assert!(result.succeeded.is_empty());
        assert!(result.failed.is_empty());
    }

    #[test]
    fn parallel_batch_failure_in_one_item_does_not_abort_others() {
        // Build a job mixing one good item with one item pointing at
        // a node that does not exist.
        let doc = doc_with_one_rect();
        let tmpdir = tempfile::tempdir().unwrap();
        let bogus_node = Uuid::new_v4();
        let job = BatchExportJob {
            id: Uuid::new_v4(),
            items: vec![
                ExportItem::Svg {
                    filename: "good.svg".into(),
                    node_ids: Vec::new(),
                    options: SvgExportOptions::default(),
                },
                ExportItem::Svg {
                    filename: "bad.svg".into(),
                    node_ids: vec![bogus_node],
                    options: SvgExportOptions::default(),
                },
            ],
            output_dir: tmpdir.path().to_path_buf(),
            status: BatchStatus::Pending,
        };
        let cancel = AtomicBool::new(false);
        let result = run_batch_parallel(&job, &doc, &RasterPixelCache::new(), &cancel, |_| {})
            .expect("parallel batch");
        assert_eq!(result.succeeded.len(), 1);
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].0, "bad.svg");
    }

    #[test]
    fn cancel_helper_is_thread_safe() {
        let c = BatchCancel::new();
        assert!(!c.is_cancelled());
        let c2 = c.clone();
        std::thread::spawn(move || c2.cancel()).join().unwrap();
        assert!(c.is_cancelled());
    }
}
