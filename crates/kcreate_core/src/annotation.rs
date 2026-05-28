//! Design-review annotations.
//!
//! Annotations are per-page collaborative comments anchored to a
//! position in world coordinates. They are intentionally simple
//! (no rich text, no embedded media) so the renderer can plot
//! them with a single pin sprite and the bridge layer can store
//! them with a single SQLite row. Threading is opt-in via
//! `thread_id`; an annotation without a parent kicks off a new
//! thread.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single review annotation. Stored in the project DB and (when
/// a collaboration session is active) broadcast to peers via the
/// `Message::AnnotationBroadcast` envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Annotation {
    pub id: Uuid,
    pub page_id: Uuid,
    pub author_peer_id: String,
    pub author_name: String,
    pub position: AnnotationPosition,
    pub text: String,
    pub timestamp: DateTime<Utc>,
    pub resolved: bool,
    /// Optional parent annotation. `None` means this is the head
    /// of a new thread; `Some(id)` means this is a reply to the
    /// referenced annotation, in which case `position` is ignored
    /// by the renderer (the reply appears in the sidebar under
    /// the head annotation).
    pub thread_id: Option<Uuid>,
}

/// World-coordinate position for an annotation pin. Stored as
/// `f64` because annotations may be placed on large artboards
/// where `f32` precision could quantise the click location.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AnnotationPosition {
    pub x: f64,
    pub y: f64,
}

impl Annotation {
    /// Construct a new top-level annotation. The caller supplies
    /// the author identity (peer id + display name) because the
    /// core crate cannot reach the collab session.
    #[must_use]
    pub fn new(
        page_id: Uuid,
        author_peer_id: impl Into<String>,
        author_name: impl Into<String>,
        position: AnnotationPosition,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            page_id,
            author_peer_id: author_peer_id.into(),
            author_name: author_name.into(),
            position,
            text: text.into(),
            timestamp: Utc::now(),
            resolved: false,
            thread_id: None,
        }
    }

    /// Construct a reply that attaches to the supplied parent
    /// annotation. Position is copied from the parent so the pin
    /// stays put when the thread head is resolved.
    #[must_use]
    pub fn reply(
        parent: &Self,
        author_peer_id: impl Into<String>,
        author_name: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            page_id: parent.page_id,
            author_peer_id: author_peer_id.into(),
            author_name: author_name.into(),
            position: parent.position,
            text: text.into(),
            timestamp: Utc::now(),
            resolved: false,
            thread_id: Some(parent.thread_id.unwrap_or(parent.id)),
        }
    }
}

/// Filter applied by the annotation overlay UI. Stored here so the
/// bridge can apply identical semantics to its list endpoint.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AnnotationFilter {
    pub include_resolved: bool,
    pub include_unresolved: bool,
}

impl AnnotationFilter {
    /// Default filter: show only unresolved annotations.
    #[must_use]
    pub const fn unresolved_only() -> Self {
        Self {
            include_resolved: false,
            include_unresolved: true,
        }
    }

    /// Show every annotation regardless of resolved status.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            include_resolved: true,
            include_unresolved: true,
        }
    }

    #[must_use]
    pub const fn matches(&self, ann: &Annotation) -> bool {
        if ann.resolved {
            self.include_resolved
        } else {
            self.include_unresolved
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_inherits_parent_thread_root() {
        let page = Uuid::new_v4();
        let head = Annotation::new(
            page,
            "peer-a",
            "Alice",
            AnnotationPosition { x: 10.0, y: 20.0 },
            "First note",
        );
        let reply = Annotation::reply(&head, "peer-b", "Bob", "Reply");
        assert_eq!(reply.thread_id, Some(head.id));
        assert_eq!(reply.position, head.position);
        assert!(!reply.resolved);
    }

    #[test]
    fn reply_to_reply_chains_to_thread_root() {
        let page = Uuid::new_v4();
        let head = Annotation::new(
            page,
            "peer-a",
            "Alice",
            AnnotationPosition { x: 0.0, y: 0.0 },
            "Head",
        );
        let mid = Annotation::reply(&head, "peer-b", "Bob", "Reply");
        let last = Annotation::reply(&mid, "peer-c", "Carol", "Reply again");
        // Replies always anchor to the thread root (mid's
        // thread_id), not to the immediate parent.
        assert_eq!(last.thread_id, Some(head.id));
    }

    #[test]
    fn filter_unresolved_excludes_resolved() {
        let mut head = Annotation::new(
            Uuid::new_v4(),
            "peer-a",
            "Alice",
            AnnotationPosition { x: 0.0, y: 0.0 },
            "Note",
        );
        let filter = AnnotationFilter::unresolved_only();
        assert!(filter.matches(&head));
        head.resolved = true;
        assert!(!filter.matches(&head));
    }

    #[test]
    fn filter_all_includes_both() {
        let mut head = Annotation::new(
            Uuid::new_v4(),
            "peer-a",
            "Alice",
            AnnotationPosition { x: 0.0, y: 0.0 },
            "Note",
        );
        let filter = AnnotationFilter::all();
        assert!(filter.matches(&head));
        head.resolved = true;
        assert!(filter.matches(&head));
    }
}
