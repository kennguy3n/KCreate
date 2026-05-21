//! Batch export.
//!
//! Runs a list of [`ExportItem`]s in sequence, writing each to a file
//! inside `output_dir`. Use this for export-preset workflows
//! ("export PNG @1x, @2x, @3x, plus SVG, plus PDF" from a single
//! click). Failures in one item do not abort the batch; they're
//! collected in the resulting [`BatchStatus::Done`].

use std::path::{Path, PathBuf};

use kcreate_core::document::DocumentGraph;
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

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::{Bounds, Node, NodeType};
    use kcreate_vector::{PathPoint, PathSegment, VectorPath};

    #[test]
    fn batch_export_writes_each_item() {
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
}
