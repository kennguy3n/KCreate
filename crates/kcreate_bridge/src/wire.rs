//! JSON wire format for scene data sent from the Electron renderer to Rust.
//!
//! Living in its own module (with no `napi`/`napi-derive` dependencies)
//! means the parser can be exercised by plain `cargo test` and reused
//! by other consumers (e.g. headless render utilities, tests).
//!
//! The schema deliberately mirrors only what [`kcreate_renderer::Scene`]
//! supports today.

use kcreate_renderer::{
    Color, Object, ObjectKind, PathCommand, Point2, Rect, Scene, Stroke, Style,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct WireScene {
    pub clear_color: [f32; 4],
    pub objects: Vec<WireObject>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WireObject {
    pub id: u64,
    pub z: i32,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub translation: [f32; 2],
    pub style: WireStyle,
    pub kind: WireKind,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WireStyle {
    pub fill: Option<[f32; 4]>,
    pub stroke: Option<WireStroke>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WireStroke {
    pub color: [f32; 4],
    pub width: f32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WireKind {
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    Circle {
        cx: f32,
        cy: f32,
        radius: f32,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    Path {
        commands: Vec<WirePathCommand>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum WirePathCommand {
    Move {
        x: f32,
        y: f32,
    },
    Line {
        x: f32,
        y: f32,
    },
    Quad {
        cx: f32,
        cy: f32,
        x: f32,
        y: f32,
    },
    Cubic {
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    },
    Close,
}

/// Errors returned by [`parse_scene`].
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("invalid scene JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Parse a JSON scene description into a renderer [`Scene`].
pub fn parse_scene(json: &str) -> Result<Scene, WireError> {
    let wire: WireScene = serde_json::from_str(json)?;
    let clear = Color::rgba(
        wire.clear_color[0],
        wire.clear_color[1],
        wire.clear_color[2],
        wire.clear_color[3],
    );
    let mut scene = Scene::new(clear);
    for obj in wire.objects {
        let kind = match obj.kind {
            WireKind::Rect {
                x,
                y,
                width,
                height,
            } => ObjectKind::Rect(Rect::new(x, y, width, height)),
            WireKind::Circle { cx, cy, radius } => ObjectKind::Circle {
                center: Point2::new(cx, cy),
                radius,
            },
            WireKind::Line { x1, y1, x2, y2 } => ObjectKind::Line {
                start: Point2::new(x1, y1),
                end: Point2::new(x2, y2),
            },
            WireKind::Path { commands } => {
                ObjectKind::Path(commands.iter().map(map_path_cmd).collect())
            }
        };
        let style = Style {
            fill: obj.style.fill.map(|c| Color::rgba(c[0], c[1], c[2], c[3])),
            stroke: obj.style.stroke.map(|s| {
                Stroke::new(
                    Color::rgba(s.color[0], s.color[1], s.color[2], s.color[3]),
                    s.width,
                )
            }),
        };
        let mut o = Object::new(kind, style)
            .with_id(kcreate_renderer::ObjectId(obj.id))
            .with_translation(obj.translation[0], obj.translation[1])
            .with_z(obj.z);
        o.visible = obj.visible;
        scene.add_object(o);
    }
    Ok(scene)
}

const fn map_path_cmd(p: &WirePathCommand) -> PathCommand {
    match *p {
        WirePathCommand::Move { x, y } => PathCommand::MoveTo(Point2::new(x, y)),
        WirePathCommand::Line { x, y } => PathCommand::LineTo(Point2::new(x, y)),
        WirePathCommand::Quad { cx, cy, x, y } => PathCommand::QuadTo {
            ctrl: Point2::new(cx, cy),
            end: Point2::new(x, y),
        },
        WirePathCommand::Cubic {
            c1x,
            c1y,
            c2x,
            c2y,
            x,
            y,
        } => PathCommand::CubicTo {
            c1: Point2::new(c1x, c1y),
            c2: Point2::new(c2x, c2y),
            end: Point2::new(x, y),
        },
        WirePathCommand::Close => PathCommand::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rect() {
        let json = r#"{
            "clear_color": [0.0, 0.0, 0.0, 1.0],
            "objects": [
                { "id": 1, "z": 0, "translation": [0,0], "style": {"fill":[1,1,1,1],"stroke":null},
                  "kind": {"type":"rect","x":0,"y":0,"width":1,"height":1} }
            ]
        }"#;
        let scene = parse_scene(json).expect("parse");
        assert_eq!(scene.objects.len(), 1);
        match &scene.objects[0].kind {
            ObjectKind::Rect(r) => assert_eq!(*r, Rect::new(0.0, 0.0, 1.0, 1.0)),
            other => panic!("unexpected kind {other:?}"),
        }
    }

    #[test]
    fn parses_circle_with_stroke_only() {
        let json = r#"{
            "clear_color": [0.0, 0.0, 0.0, 1.0],
            "objects": [
                { "id": 7, "z": 2, "translation": [3,4], "style": {"fill":null,"stroke":{"color":[1,0,0,1],"width":2}},
                  "kind": {"type":"circle","cx":10,"cy":20,"radius":5} }
            ]
        }"#;
        let scene = parse_scene(json).expect("parse");
        assert_eq!(scene.objects[0].id.0, 7);
        assert_eq!(scene.objects[0].translation, (3.0, 4.0));
        assert!(scene.objects[0].style.stroke.is_some());
        assert!(scene.objects[0].style.fill.is_none());
    }

    #[test]
    fn parses_path_with_all_command_kinds() {
        let json = r#"{
            "clear_color": [0,0,0,1],
            "objects": [
                { "id":1, "z":0, "translation":[0,0], "style":{"fill":[1,0,0,1],"stroke":null},
                  "kind":{"type":"path","commands":[
                    {"op":"move","x":0,"y":0},
                    {"op":"line","x":1,"y":1},
                    {"op":"quad","cx":2,"cy":2,"x":3,"y":3},
                    {"op":"cubic","c1x":4,"c1y":4,"c2x":5,"c2y":5,"x":6,"y":6},
                    {"op":"close"}
                  ]} }
            ]
        }"#;
        let scene = parse_scene(json).expect("parse");
        match &scene.objects[0].kind {
            ObjectKind::Path(cmds) => assert_eq!(cmds.len(), 5),
            other => panic!("expected path got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_scene("not-json").is_err());
    }

    #[test]
    fn missing_translation_defaults_to_zero() {
        let json = r#"{
            "clear_color": [0,0,0,1],
            "objects": [
                { "id":1, "z":0, "style":{"fill":[1,1,1,1],"stroke":null},
                  "kind":{"type":"rect","x":0,"y":0,"width":1,"height":1} }
            ]
        }"#;
        let scene = parse_scene(json).expect("parse");
        assert_eq!(scene.objects[0].translation, (0.0, 0.0));
    }
}
