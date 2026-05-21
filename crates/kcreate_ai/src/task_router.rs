//! Task router: a single entry point that the bridge calls
//! regardless of which model / algorithm backs the task.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bg_remove::{remove_background, BgRemoveError, BgRemoveOptions};

/// Errors from [`execute_task`].
#[derive(Debug, Error)]
pub enum AiError {
    #[error(transparent)]
    BgRemove(#[from] BgRemoveError),
    #[error("unsupported task: {0}")]
    Unsupported(String),
}

/// One AI task. Discriminated externally for cheap JSON crossings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiTask {
    BackgroundRemoval {
        image_data: Vec<u8>,
        width: u32,
        height: u32,
        #[serde(default)]
        tolerance: Option<u8>,
        #[serde(default)]
        feather: Option<u8>,
    },
}

/// One AI result. Discriminated externally so the bridge can decide
/// how to write the output back into the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiResult {
    BackgroundRemoval {
        /// Single-channel mask (255 = subject, 0 = background). Same
        /// dimensions as the input.
        mask: Vec<u8>,
        /// New RGBA8 buffer with alpha modulated by the mask.
        output_rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
}

/// Execute one task synchronously on the calling thread.
pub fn execute_task(task: AiTask) -> Result<AiResult, AiError> {
    match task {
        AiTask::BackgroundRemoval {
            image_data,
            width,
            height,
            tolerance,
            feather,
        } => {
            let mut opts = BgRemoveOptions::default();
            if let Some(t) = tolerance {
                opts.tolerance = t;
            }
            if let Some(f) = feather {
                opts.feather = f;
            }
            let out = remove_background(&image_data, width, height, opts)?;
            let mask: Vec<u8> = out.chunks_exact(4).map(|p| p[3]).collect();
            Ok(AiResult::BackgroundRemoval {
                mask,
                output_rgba: out,
                width,
                height,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_bg_removal() {
        let mut img = vec![240u8; 16 * 16 * 4];
        for px in img.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }
        // Make a non-background pixel in the centre.
        let centre = (8 * 16 + 8) * 4;
        img[centre] = 10;
        img[centre + 1] = 10;
        img[centre + 2] = 10;
        let result = execute_task(AiTask::BackgroundRemoval {
            image_data: img,
            width: 16,
            height: 16,
            tolerance: Some(20),
            feather: Some(8),
        })
        .expect("ok");
        let AiResult::BackgroundRemoval {
            mask, output_rgba, ..
        } = result;
        assert_eq!(mask.len(), 16 * 16);
        assert_eq!(output_rgba.len(), 16 * 16 * 4);
        assert_eq!(output_rgba[3], 0);
    }
}
