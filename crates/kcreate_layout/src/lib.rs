//! Pure-Rust auto-layout solver for KCreate frames.
//!
//! Two layout modes are exposed: a flexbox-style row/column solver
//! (`flex.rs`) and a uniform CSS-grid (`grid.rs`). Both take a parent
//! rect, a list of child sizes, and a layout config; both return
//! `(child_id, new_bounds)` pairs. No DOM, no side effects, no
//! dependency on the renderer — these functions are deterministic and
//! safe to call from any thread.

pub mod flex;
pub mod grid;
pub mod padding;

pub use flex::{layout_flex, Alignment, CrossAlignment, FlexDirection, FlexLayout};
pub use grid::{layout_grid, GridLayout};
pub use padding::Padding;
