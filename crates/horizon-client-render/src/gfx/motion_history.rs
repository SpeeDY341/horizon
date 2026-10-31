//! Previous *rendered* geometry, keyed by entity rather than batch position.
use super::Vertex;
use glam::{DVec3, Vec3};
use std::{collections::HashMap, ops::Range};

/// Independent renderers may use the same entity ID or block position.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Source {
    Mob,
    Item,
    Player,
    Guest,
    Boat,
    Cart,
    FallingBlock,
    Tnt,
    ArmorStand,
    Projectile,
    Trident,
    Rocket,
    Potion,
    Fangs,
    Display,
    Crystal,
    Eye,
    BlockEntity,
    Piston,
    Spawner,
    Vault,
    Brushable,
    OminousSpawner,
    Leash,
    Frame,
    FrameItem,
    FrameMap,
    FishingHook,
    Apparition,
    Painting,
    Cushion,
    DragonFireball,
    MobBeam,
    DragonBeam,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Key {
    /// The UUID distinguishes a network ID reused between rendered frames.
    /// Locally allocated IDs are monotonic for the lifetime of a level.
    Entity(Source, u32, Option<[i32; 4]>),
    Block(Source, [i32; 3]),
}

/// Visible geometry in `Frame::entities`, before material sorting.
#[derive(Clone, Debug)]
pub struct Span {
    pub key: Key,
    pub vertices: Range<usize>,
}

impl Span {
    /// The production entity registration path, also used by the GPU regression.
    pub fn entity(
        spans: &mut Vec<Self>,
        source: Source,
        id: u32,
        uuid: Option<[i32; 4]>,
        start: usize,
        end: usize,
    ) {
        Self::record(spans, Key::Entity(source, id, uuid), start, end);
    }

    pub fn record(spans: &mut Vec<Self>, key: Key, start: usize, end: usize) {
        if end > start {
            spans.push(Self {
                key,
                vertices: start..end,
            });
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MotionVertex {
    pub current: [f32; 3],
    pub previous: [f32; 3],
    pub uv: [f32; 2],
    pub alpha: f32,
    pub cull: f32,
}

struct Geometry {
    vertices: Vec<Vertex>,
}

#[derive(Default)]
pub struct History {
    previous: HashMap<Key, Geometry>,
    camera: Option<DVec3>,
    pub vertices: Vec<MotionVertex>,
}

impl History {
    pub fn clear(&mut self) {
        self.previous.clear();
        self.vertices.clear();
        self.camera = None;
    }

    pub fn update(&mut self, spans: &[Span], vertices: &[Vertex], camera: DVec3, reset: bool) {
        if reset {
            self.clear();
        }
        self.vertices.clear();
        let delta = (camera - self.camera.unwrap_or(camera)).as_vec3();
        let mut next = HashMap::with_capacity(spans.len());
        for span in spans {
            let Some(current) = vertices.get(span.vertices.clone()) else {
                continue;
            };
            if current.is_empty() || current.len() % 4 != 0 {
                continue;
            }
            let key = span.key;
            let previous = self.previous.remove(&key);
            let previous = previous
                .as_ref()
                .filter(|p| compatible(&p.vertices, current, delta));
            self.vertices
                .extend(current.iter().enumerate().map(|(i, v)| MotionVertex {
                    current: v.pos,
                    // New/reappearing geometry has camera motion only. Positions in the
                    // history already use the previous camera origin, including animation.
                    previous: previous.map_or_else(
                        || (Vec3::from_array(v.pos) + delta).to_array(),
                        |p| p.vertices[i].pos,
                    ),
                    uv: v.uv,
                    alpha: (v.color >> 24) as f32 / 255.0,
                    cull: if v.normal[3] == 1 { 1.0 } else { 0.0 },
                }));
            next.insert(
                key,
                Geometry {
                    vertices: current.to_vec(),
                },
            );
        }
        // Not rendered this frame means no history next frame; IDs may be reused.
        self.previous = next;
        self.camera = Some(camera);
    }
}

fn compatible(previous: &[Vertex], current: &[Vertex], delta: Vec3) -> bool {
    previous.len() == current.len()
        && previous.iter().zip(current).all(|(p, c)| {
            p.uv == c.uv
                && p.layer == c.layer
                && (Vec3::from_array(p.pos) - Vec3::from_array(c.pos) - delta).length_squared()
                    < 1024.0
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x: f32) -> Vec<Vertex> {
        (0..4)
            .map(|i| Vertex {
                pos: [x + (i % 2) as f32, (i / 2) as f32, -3.0],
                uv: [(i % 2) as f32, (i / 2) as f32],
                color: u32::MAX,
                ..bytemuck::Zeroable::zeroed()
            })
            .collect()
    }

    fn span(id: u32, start: usize) -> Span {
        Span {
            key: Key::Entity(Source::Mob, id, None),
            vertices: start..start + 4,
        }
    }

    #[test]
    fn reorder_and_animation_keep_the_same_entity_vertices() {
        let mut history = History::default();
        let mut a = quad(1.0);
        a.extend(quad(10.0));
        history.update(&[span(1, 0), span(2, 4)], &a, DVec3::ZERO, false);
        let mut b = quad(11.0);
        b.extend(quad(2.0));
        b[5].pos[1] += 0.25; // animated limb, not just root translation
        history.update(&[span(2, 0), span(1, 4)], &b, DVec3::ZERO, false);
        assert_eq!(history.vertices[0].previous, a[4].pos);
        assert_eq!(history.vertices[5].previous, a[1].pos);
        assert_ne!(history.vertices[5].current, history.vertices[5].previous);
    }

    #[test]
    fn new_hidden_or_changed_geometry_does_not_borrow_old_motion() {
        let mut h = History::default();
        h.update(&[span(1, 0)], &quad(1.0), DVec3::ZERO, false);
        h.update(&[], &[], DVec3::X, false);
        h.update(&[span(1, 0)], &quad(3.0), DVec3::X * 2.0, false);
        assert_eq!(h.vertices[0].previous[0], 4.0);
        let mut changed = quad(4.0);
        changed[0].uv[0] = 0.25;
        h.update(&[span(1, 0)], &changed, DVec3::X * 2.0, false);
        assert_eq!(h.vertices[0].previous, h.vertices[0].current);
        h.update(&[span(1, 0)], &quad(5.0), DVec3::X * 2.0, true);
        assert_eq!(h.vertices[0].previous, h.vertices[0].current);
    }

    #[test]
    fn renderer_namespaces_and_reused_network_ids_never_share_history() {
        let mut h = History::default();
        let mut spans = Vec::new();
        Span::entity(&mut spans, Source::Boat, 7, Some([1; 4]), 0, 4);
        Span::entity(&mut spans, Source::Player, 7, Some([1; 4]), 4, 8);
        let mut first = quad(1.0);
        first.extend(quad(10.0));
        h.update(&spans, &first, DVec3::ZERO, false);
        // Same IDs, reversed builders. The boat is a new network incarnation.
        spans.clear();
        Span::entity(&mut spans, Source::Player, 7, Some([1; 4]), 0, 4);
        Span::entity(&mut spans, Source::Boat, 7, Some([2; 4]), 4, 8);
        let mut next = quad(11.0);
        next.extend(quad(2.0));
        h.update(&spans, &next, DVec3::ZERO, false);
        assert_eq!(h.vertices[0].previous, first[4].pos);
        assert_eq!(h.vertices[4].previous, next[4].pos);
    }

    #[test]
    fn changed_topology_and_disappearing_objects_fall_back_to_camera_motion() {
        let mut h = History::default();
        let mut geometry = quad(1.0);
        geometry.extend(quad(2.0));
        let mut s = span(1, 0);
        s.vertices = 0..8;
        h.update(&[s], &geometry, DVec3::ZERO, false);
        h.update(&[span(1, 0)], &quad(3.0), DVec3::X, false);
        assert_eq!(h.vertices[0].previous[0], 4.0);
        h.update(&[], &[], DVec3::X, false);
        assert!(h.vertices.is_empty());
        h.update(&[span(1, 0)], &quad(4.0), DVec3::X * 2.0, false);
        assert_eq!(h.vertices[0].previous[0], 5.0);
    }

    #[test]
    fn camera_relative_history_preserves_precision_far_from_spawn() {
        let mut h = History::default();
        let camera = DVec3::new(25_000_000.25, 80.0, 25_000_000.75);
        h.update(&[span(1, 0)], &quad(1.0), camera, false);
        h.update(&[span(1, 0)], &quad(0.75), camera + DVec3::X * 0.25, false);
        assert_eq!(h.vertices[0].previous[0], 1.0);
    }
}
