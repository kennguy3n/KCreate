//! Phase 8 Block C: brand kit versioning.
//!
//! Tests the save / list / restore / diff round-trip for brand
//! kit snapshots stored in the `brand_kit_versions` SQLite table.

use kcreate_core::node::RgbaColor;
use kcreate_core::project::{BrandKit, NamedColor};
use kcreate_storage::brand_versions::{
    diff_brand_kit_versions, list_brand_kit_versions, restore_brand_kit_version,
    save_brand_kit_version,
};
use kcreate_storage::Database;

fn make_brand_kit(primary: RgbaColor) -> BrandKit {
    let mut kit = BrandKit::new("Test Brand");
    kit.colors.push(NamedColor {
        name: "Primary".into(),
        color: primary,
    });
    kit
}

#[test]
fn save_and_list_versions_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("brand.db")).unwrap();
    let conn = db.conn();
    let kit = make_brand_kit(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
    save_brand_kit_version(conn, &kit, "initial").unwrap();
    let versions = list_brand_kit_versions(conn, kit.id).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].description, "initial");
    assert_eq!(versions[0].brand_kit_id, kit.id);
}

#[test]
fn restore_returns_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("brand.db")).unwrap();
    let conn = db.conn();
    let mut kit = make_brand_kit(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
    let v1 = save_brand_kit_version(conn, &kit, "v1").unwrap();
    kit.colors[0].color = RgbaColor::new(0.0, 0.0, 1.0, 1.0);
    save_brand_kit_version(conn, &kit, "v2").unwrap();
    let restored = restore_brand_kit_version(conn, v1.version_id).unwrap();
    assert!(
        (restored.colors[0].color.r - 1.0).abs() < 1e-6,
        "should restore to the v1 (red) snapshot"
    );
}

#[test]
fn diff_detects_added_and_changed_colors() {
    let before = make_brand_kit(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
    let mut after = before.clone();
    after.colors[0].color = RgbaColor::new(0.0, 1.0, 0.0, 1.0); // changed
    after.colors.push(NamedColor {
        name: "Secondary".into(),
        color: RgbaColor::new(0.0, 0.0, 1.0, 1.0),
    });
    let diff = diff_brand_kit_versions(&before, &after);
    assert_eq!(diff.added_colors.len(), 1);
    assert_eq!(diff.added_colors[0].name, "Secondary");
    assert_eq!(diff.changed_colors.len(), 1);
    assert_eq!(diff.changed_colors[0].name, "Primary");
    assert!(diff.removed_colors.is_empty());
}

#[test]
fn diff_detects_removed_colors() {
    let mut before = make_brand_kit(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
    before.colors.push(NamedColor {
        name: "Secondary".into(),
        color: RgbaColor::new(0.0, 0.0, 1.0, 1.0),
    });
    let mut after = before.clone();
    after.colors.pop();
    let diff = diff_brand_kit_versions(&before, &after);
    assert_eq!(diff.removed_colors.len(), 1);
    assert_eq!(diff.removed_colors[0].name, "Secondary");
}

#[test]
fn diff_detects_name_change() {
    let before = make_brand_kit(RgbaColor::new(1.0, 0.0, 0.0, 1.0));
    let mut after = before.clone();
    after.name = "Renamed Brand".into();
    let diff = diff_brand_kit_versions(&before, &after);
    assert!(diff.name_changed.is_some());
    let (b, a) = diff.name_changed.unwrap();
    assert_eq!(b, "Test Brand");
    assert_eq!(a, "Renamed Brand");
}
