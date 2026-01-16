//! Loads a Cocos Creator 2.x `.fire` scene (JSON) and renders it with Bevy.
//!
//! This is a first milestone focused on matching visual layout for a small subset:
//! - `cc.Node` (transform + hierarchy)
//! - `cc.Sprite` (image)
//! - `cc.Label` (text)
//! - `cc.Canvas` / `cc.Widget` (design resolution / stretch-to-screen metadata)
//!
//! Assets are loaded from `assets-cocos/` via a dedicated asset source (`cocos://`).

use anyhow::{Context as _, Result};
use bevy::{
    asset::{
        io::{AssetSourceBuilder, AssetSourceId},
        AssetPath,
    },
    ecs::hierarchy::ChildSpawnerCommands,
    prelude::*,
    sprite::Anchor,
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const COCOS_ASSET_SOURCE: &str = "cocos";

fn main() {
    let asset_root = PathBuf::from("assets-cocos");
    let fire_path = asset_root.join("game.fire");
    let project = load_cocos_project(&fire_path, &asset_root)
        .unwrap_or_else(|e| panic!("Failed to load Cocos project from {fire_path:?}: {e:#}"));

    App::new()
        // Mount `assets-cocos/` as `cocos://...` without changing Bevy's default `assets/` source.
        .register_asset_source(
            COCOS_ASSET_SOURCE,
            AssetSourceBuilder::platform_default("assets-cocos", None),
        )
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Cocos .fire scene (Bevy)".into(),
                resolution: (
                    project.design_resolution.x as u32,
                    project.design_resolution.y as u32,
                )
                    .into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(project)
        .insert_resource(ClearColor(Color::BLACK))
        .add_systems(Startup, setup)
        .run();
}

#[derive(Component, Debug, Clone, Copy)]
#[allow(dead_code)]
struct CocosId(usize);

#[derive(Resource)]
struct CocosProject {
    design_resolution: Vec2,
    root_children: Vec<usize>,
    nodes: HashMap<usize, CocosNode>,
    sprite_frames: HashMap<String, SpriteFrameInfo>,
}

#[derive(Debug, Clone)]
struct CocosNode {
    name: String,
    #[allow(dead_code)]
    parent: Option<usize>,
    children: Vec<usize>,
    position: Vec2,
    rotation_z_radians: f32,
    scale: Vec2,
    size: Vec2,
    anchor: Vec2,
    color: Color,

    sprite: Option<CocosSprite>,
    label: Option<CocosLabel>,
    canvas: Option<CocosCanvas>,
    widget: Option<CocosWidget>,
}

#[derive(Debug, Clone)]
struct CocosSprite {
    sprite_frame_uuid: String,
    #[allow(dead_code)]
    atlas_uuid: Option<String>,
}

#[derive(Debug, Clone)]
struct CocosLabel {
    text: String,
    font_size: f32,
    #[allow(dead_code)]
    font_family: String,
    horizontal_align: i64,
    vertical_align: i64,
}

#[derive(Debug, Clone)]
struct CocosCanvas {
    design_resolution: Vec2,
    #[allow(dead_code)]
    fit_width: bool,
    #[allow(dead_code)]
    fit_height: bool,
}

#[derive(Debug, Clone)]
struct CocosWidget {
    #[allow(dead_code)]
    align_flags: u32,
    #[allow(dead_code)]
    left: f32,
    #[allow(dead_code)]
    right: f32,
    #[allow(dead_code)]
    top: f32,
    #[allow(dead_code)]
    bottom: f32,
}

#[derive(Debug, Clone)]
struct SpriteFrameInfo {
    image_path: String,
    rect_min: UVec2,
    rect_size: UVec2,
    rotated: bool,
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>, project: Res<CocosProject>) {
    commands.spawn(Camera2d);

    let font: Handle<Font> = asset_server.load("fonts/FiraSans-Bold.ttf");

    // Cocos scenes in this project use a bottom-left origin. Bevy 2D uses a centered origin.
    // Shifting the whole hierarchy makes Cocos (0,0) map to the bottom-left of the Bevy camera.
    let root_shift = Vec3::new(
        -project.design_resolution.x / 2.0,
        -project.design_resolution.y / 2.0,
        0.0,
    );

    commands
        .spawn((
            Name::new("CocosRoot"),
            Transform::from_translation(root_shift),
            GlobalTransform::default(),
        ))
        .with_children(|root| {
            for &child in &project.root_children {
                spawn_node_recursive(root, child, &project, &asset_server, &font);
            }
        });
}

fn spawn_node_recursive(
    parent: &mut ChildSpawnerCommands<'_>,
    node_id: usize,
    project: &CocosProject,
    asset_server: &AssetServer,
    font: &Handle<Font>,
) {
    let Some(node) = project.nodes.get(&node_id) else {
        warn!("Missing cc.Node object for __id__={node_id}");
        return;
    };

    let node_transform = Transform::from_translation(Vec3::new(node.position.x, node.position.y, 0.0))
        .with_rotation(Quat::from_rotation_z(node.rotation_z_radians))
        .with_scale(Vec3::new(node.scale.x, node.scale.y, 1.0));

    parent
        .spawn((
            Name::new(node.name.clone()),
            CocosId(node_id),
            node_transform,
            GlobalTransform::default(),
        ))
        .with_children(|children| {
            if let Some(sprite) = &node.sprite {
                spawn_sprite(children, node, sprite, project, asset_server);
            }
            if let Some(label) = &node.label {
                spawn_label(children, node, label, font);
            }

            for &child in &node.children {
                spawn_node_recursive(children, child, project, asset_server, font);
            }
        });
}

fn spawn_sprite(
    parent: &mut ChildSpawnerCommands<'_>,
    node: &CocosNode,
    sprite: &CocosSprite,
    project: &CocosProject,
    asset_server: &AssetServer,
) {
    let node_anchor = Anchor(Vec2::new(node.anchor.x - 0.5, node.anchor.y - 0.5));
    let mut render_transform = Transform::default();
    let mut anchor = node_anchor;

    let Some(frame) = project.sprite_frames.get(&sprite.sprite_frame_uuid) else {
        // Unknown sprite frame: use a visible placeholder box matching the node size.
        parent.spawn((
            Name::new("Sprite(unknown)"),
            Sprite::from_color(
                Color::srgba(1.0, 0.0, 1.0, node.color.to_srgba().alpha),
                node.size,
            ),
            anchor,
        ));
        return;
    };

    let source = AssetSourceId::from(COCOS_ASSET_SOURCE);
    let image_path = AssetPath::from_path(Path::new(&frame.image_path)).with_source(source);
    let image: Handle<Image> = asset_server.load(image_path);

    let rect_min = Vec2::new(frame.rect_min.x as f32, frame.rect_min.y as f32);
    let rect_max = rect_min + Vec2::new(frame.rect_size.x as f32, frame.rect_size.y as f32);

    let mut sprite_component = Sprite::from_image(image);
    sprite_component.rect = Some(Rect::from_corners(rect_min, rect_max));
    sprite_component.color = node.color;

    // Match node size. For rotated atlas frames, Cocos uses UV rotation without affecting the node's transform.
    // We emulate this by rotating only the render entity, swapping the local size, and compensating the anchor.
    if node.size != Vec2::ZERO {
        sprite_component.custom_size = Some(if frame.rotated {
            Vec2::new(node.size.y, node.size.x)
        } else {
            node.size
        });
    }

    if frame.rotated {
        render_transform.rotation = Quat::from_rotation_z(-core::f32::consts::FRAC_PI_2);
        // Compensate anchor rotation so the node's anchor still behaves in node-local space.
        anchor = Anchor(Vec2::new(-node_anchor.0.y, node_anchor.0.x));
    }

    parent.spawn((Name::new("Sprite"), sprite_component, anchor, render_transform));
}

fn spawn_label(
    parent: &mut ChildSpawnerCommands<'_>,
    node: &CocosNode,
    label: &CocosLabel,
    font: &Handle<Font>,
) {
    let justify = match label.horizontal_align {
        2 => Justify::Right,
        1 => Justify::Center,
        _ => Justify::Left,
    };

    // Vertical alignment is ignored for now. In Cocos it affects layout within the node's bounds.
    let _ = label.vertical_align;

    parent.spawn((
        Name::new("Label"),
        Text2d::new(label.text.clone()),
        TextFont {
            font: font.clone().into(),
            font_size: label.font_size,
            ..default()
        },
        TextColor(node.color),
        TextLayout::new_with_justify(justify),
        Anchor(Vec2::new(node.anchor.x - 0.5, node.anchor.y - 0.5)),
        Transform::default(),
    ));
}

fn load_cocos_project(fire_path: &Path, asset_root: &Path) -> Result<CocosProject> {
    let fire = fs::read_to_string(fire_path)
        .with_context(|| format!("reading {}", fire_path.display()))?;
    let objects: Vec<Value> =
        serde_json::from_str(&fire).with_context(|| format!("parsing {}", fire_path.display()))?;

    let root_children = find_scene_root_children(&objects)
        .with_context(|| format!("finding cc.Scene _children in {}", fire_path.display()))?;

    let mut nodes = HashMap::new();
    for (id, obj) in objects.iter().enumerate() {
        if obj_type(obj) != Some("cc.Node") {
            continue;
        }
        nodes.insert(id, parse_node(obj)?);
    }

    // Attach supported component data to nodes.
    for obj in &objects {
        match obj_type(obj) {
            Some("cc.Sprite") => {
                let node_id = get_id(obj, "node.__id__")?;
                let sprite_frame_uuid = get_string(obj, "_spriteFrame.__uuid__")?;
                let atlas_uuid = get_opt_string(obj, "_atlas.__uuid__");
                if let Some(node) = nodes.get_mut(&node_id) {
                    node.sprite = Some(CocosSprite {
                        sprite_frame_uuid,
                        atlas_uuid,
                    });
                }
            }
            Some("cc.Label") => {
                let node_id = get_id(obj, "node.__id__")?;
                if let Some(node) = nodes.get_mut(&node_id) {
                    node.label = Some(CocosLabel {
                        text: get_string(obj, "_N$string").unwrap_or_else(|_| "Label".into()),
                        font_size: get_f32(obj, "_fontSize").unwrap_or(40.0),
                        font_family: get_string(obj, "_N$fontFamily").unwrap_or_else(|_| "Arial".into()),
                        horizontal_align: get_i64(obj, "_N$horizontalAlign").unwrap_or(1),
                        vertical_align: get_i64(obj, "_N$verticalAlign").unwrap_or(1),
                    });
                }
            }
            Some("cc.Canvas") => {
                let node_id = get_id(obj, "node.__id__")?;
                if let Some(node) = nodes.get_mut(&node_id) {
                    node.canvas = Some(CocosCanvas {
                        design_resolution: Vec2::new(
                            get_f32(obj, "_designResolution.width").unwrap_or(960.0),
                            get_f32(obj, "_designResolution.height").unwrap_or(640.0),
                        ),
                        fit_width: get_bool(obj, "_fitWidth").unwrap_or(false),
                        fit_height: get_bool(obj, "_fitHeight").unwrap_or(true),
                    });
                }
            }
            Some("cc.Widget") => {
                let node_id = get_id(obj, "node.__id__")?;
                if let Some(node) = nodes.get_mut(&node_id) {
                    node.widget = Some(CocosWidget {
                        align_flags: get_u32(obj, "_alignFlags").unwrap_or(0),
                        left: get_f32(obj, "_left").unwrap_or(0.0),
                        right: get_f32(obj, "_right").unwrap_or(0.0),
                        top: get_f32(obj, "_top").unwrap_or(0.0),
                        bottom: get_f32(obj, "_bottom").unwrap_or(0.0),
                    });
                }
            }
            _ => {}
        }
    }

    // Prefer the Canvas design resolution if present.
    let design_resolution = nodes
        .values()
        .find_map(|n| n.canvas.as_ref().map(|c| c.design_resolution))
        .unwrap_or(Vec2::new(960.0, 640.0));

    let sprite_frames = build_sprite_frame_registry(asset_root)
        .with_context(|| format!("building sprite-frame registry from {}", asset_root.display()))?;

    Ok(CocosProject {
        design_resolution,
        root_children,
        nodes,
        sprite_frames,
    })
}

fn find_scene_root_children(objects: &[Value]) -> Result<Vec<usize>> {
    for obj in objects {
        if obj_type(obj) != Some("cc.Scene") {
            continue;
        }
        let children = obj
            .get("_children")
            .and_then(Value::as_array)
            .context("cc.Scene._children is missing or not an array")?;

        let mut ids = Vec::with_capacity(children.len());
        for child in children {
            let id = child
                .get("__id__")
                .and_then(Value::as_u64)
                .context("cc.Scene child missing __id__")? as usize;
            ids.push(id);
        }
        return Ok(ids);
    }
    anyhow::bail!("no cc.Scene found");
}

fn parse_node(obj: &Value) -> Result<CocosNode> {
    let name = obj
        .get("_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let parent = obj
        .get("_parent")
        .and_then(|p| p.get("__id__"))
        .and_then(Value::as_u64)
        .map(|v| v as usize);

    let children = obj
        .get("_children")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.get("__id__").and_then(Value::as_u64).map(|x| x as usize))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let size = Vec2::new(
        get_f32(obj, "_contentSize.width").unwrap_or(0.0),
        get_f32(obj, "_contentSize.height").unwrap_or(0.0),
    );

    let anchor = Vec2::new(
        get_f32(obj, "_anchorPoint.x").unwrap_or(0.5),
        get_f32(obj, "_anchorPoint.y").unwrap_or(0.5),
    );

    let (position, scale) = parse_trs(obj);
    let rotation_z_radians = get_f32(obj, "_eulerAngles.z")
        .unwrap_or(0.0)
        .to_radians();

    let (r, g, b, a) = parse_color(obj);
    let opacity = get_f32(obj, "_opacity").unwrap_or(255.0).clamp(0.0, 255.0) / 255.0;
    let color = Color::srgba(
        r / 255.0,
        g / 255.0,
        b / 255.0,
        (a / 255.0) * opacity,
    );

    Ok(CocosNode {
        name,
        parent,
        children,
        position,
        rotation_z_radians,
        scale,
        size,
        anchor,
        color,
        sprite: None,
        label: None,
        canvas: None,
        widget: None,
    })
}

fn parse_trs(obj: &Value) -> (Vec2, Vec2) {
    let Some(array) = obj
        .get("_trs")
        .and_then(|trs| trs.get("array"))
        .and_then(Value::as_array)
    else {
        return (Vec2::ZERO, Vec2::ONE);
    };

    // Cocos Creator 2.x stores TRS as:
    // [posX, posY, posZ, rotX, rotY, rotZ, rotW, scaleX, scaleY, scaleZ]
    let x = array.get(0).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let y = array.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let sx = array.get(7).and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let sy = array.get(8).and_then(Value::as_f64).unwrap_or(1.0) as f32;

    (Vec2::new(x, y), Vec2::new(sx, sy))
}

fn parse_color(obj: &Value) -> (f32, f32, f32, f32) {
    let Some(c) = obj.get("_color") else {
        return (255.0, 255.0, 255.0, 255.0);
    };
    let r = get_f32(c, "r").unwrap_or(255.0);
    let g = get_f32(c, "g").unwrap_or(255.0);
    let b = get_f32(c, "b").unwrap_or(255.0);
    let a = get_f32(c, "a").unwrap_or(255.0);
    (r, g, b, a)
}

fn build_sprite_frame_registry(asset_root: &Path) -> Result<HashMap<String, SpriteFrameInfo>> {
    let mut registry = HashMap::new();
    let mut meta_files = Vec::new();
    collect_meta_files(asset_root, &mut meta_files)?;

    for meta_path in meta_files {
        let text = fs::read_to_string(&meta_path)
            .with_context(|| format!("reading {}", meta_path.display()))?;

        if meta_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".plist.meta"))
        {
            let parsed: MetaWithSubMetas =
                serde_json::from_str(&text).with_context(|| meta_path.display().to_string())?;
            let image_rel = derive_image_rel_path(asset_root, &meta_path)?;
            register_sub_metas(&mut registry, image_rel, parsed.sub_metas);
        } else if meta_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".png.meta"))
        {
            let parsed: MetaWithSubMetas =
                serde_json::from_str(&text).with_context(|| meta_path.display().to_string())?;
            if parsed.sub_metas.is_empty() {
                continue;
            }
            let image_rel = derive_image_rel_path(asset_root, &meta_path)?;
            register_sub_metas(&mut registry, image_rel, parsed.sub_metas);
        }
    }

    Ok(registry)
}

fn register_sub_metas(
    registry: &mut HashMap<String, SpriteFrameInfo>,
    image_path: String,
    sub_metas: HashMap<String, SpriteFrameMeta>,
) {
    for (_name, frame) in sub_metas {
        let rect_min = UVec2::new(frame.trim_x, frame.trim_y);
        let rect_size = if frame.rotated {
            UVec2::new(frame.height, frame.width)
        } else {
            UVec2::new(frame.width, frame.height)
        };

        let info = SpriteFrameInfo {
            image_path: image_path.clone(),
            rect_min,
            rect_size,
            rotated: frame.rotated,
        };

        if registry.insert(frame.uuid.clone(), info).is_some() {
            warn!("Duplicate sprite-frame uuid detected: {}", frame.uuid);
        }
    }
}

fn collect_meta_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_meta_files(&path, out)?;
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        out.push(path);
    }
    Ok(())
}

fn derive_image_rel_path(asset_root: &Path, meta_path: &Path) -> Result<String> {
    //  - `foo.png.meta`   -> `foo.png`
    //  - `foo.plist.meta` -> `foo.png` (same basename as the atlas)
    let without_meta = meta_path.with_extension("");
    let image_path = if without_meta
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("plist"))
    {
        without_meta.with_extension("png")
    } else {
        without_meta
    };

    let rel = image_path
        .strip_prefix(asset_root)
        .with_context(|| format!("meta file is outside asset root: {}", meta_path.display()))?;

    Ok(rel.to_string_lossy().replace('\\', "/"))
}

#[derive(Debug, Deserialize)]
struct MetaWithSubMetas {
    #[serde(rename = "subMetas", default)]
    sub_metas: HashMap<String, SpriteFrameMeta>,
}

#[derive(Debug, Deserialize)]
struct SpriteFrameMeta {
    uuid: String,
    #[serde(default)]
    rotated: bool,
    #[serde(rename = "trimX", default)]
    trim_x: u32,
    #[serde(rename = "trimY", default)]
    trim_y: u32,
    width: u32,
    height: u32,
}

fn obj_type(obj: &Value) -> Option<&str> {
    obj.get("__type__")?.as_str()
}

fn get_bool(obj: &Value, path: &str) -> Result<bool> {
    obj_get(obj, path)
        .and_then(Value::as_bool)
        .with_context(|| format!("expected bool at {path}"))
}

fn get_u32(obj: &Value, path: &str) -> Result<u32> {
    obj_get(obj, path)
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .with_context(|| format!("expected u32 at {path}"))
}

fn get_i64(obj: &Value, path: &str) -> Result<i64> {
    obj_get(obj, path)
        .and_then(Value::as_i64)
        .with_context(|| format!("expected i64 at {path}"))
}

fn get_f32(obj: &Value, path: &str) -> Result<f32> {
    obj_get(obj, path)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .with_context(|| format!("expected f32 at {path}"))
}

fn get_id(obj: &Value, path: &str) -> Result<usize> {
    obj_get(obj, path)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .with_context(|| format!("expected __id__ at {path}"))
}

fn get_string(obj: &Value, path: &str) -> Result<String> {
    obj_get(obj, path)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("expected string at {path}"))
}

fn get_opt_string(obj: &Value, path: &str) -> Option<String> {
    obj_get(obj, path).and_then(Value::as_str).map(str::to_string)
}

fn obj_get<'a>(obj: &'a Value, dotted_path: &str) -> Option<&'a Value> {
    let mut cur = obj;
    for part in dotted_path.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}
