//! Keep live windows in a row and fit their compact history beneath it.
use super::canvas::{HEIGHT, WIDTH};
use crate::model::{Node, Position, State};
use std::collections::HashMap;

pub const CLOSED_HEIGHT: f64 = 78.0;
const LIVE_STEP: f64 = 340.0;
const HISTORY_STEP: f64 = 320.0;
const HISTORY_ROW: f64 = 100.0;
const SECTION_GAP: f64 = 60.0;

type Key = (String, bool, i32, i32, Vec<(String, Option<u64>)>);

#[derive(Clone, Default)]
pub struct Arrangement {
    key: Option<Key>,
    offsets: HashMap<String, Position>,
}

fn managed(node: &Node) -> bool {
    node.desktop_window || node.closed_window()
}

impl Arrangement {
    fn key(state: &State, width: i32, height: i32) -> Key {
        (
            state.selected_workspace.clone(),
            state.workspace().graph_arranged,
            width,
            height,
            state
                .nodes
                .iter()
                .filter(|n| n.workspace == state.selected_workspace && managed(n))
                .map(|n| (n.id.clone(), n.window_id))
                .collect(),
        )
    }

    pub fn changed(&self, state: &State, width: i32, height: i32) -> bool {
        self.key.as_ref() != Some(&Self::key(state, width, height))
    }

    pub fn position(&self, node: &Node) -> Position {
        let offset = self.offsets.get(&node.id).cloned().unwrap_or_default();
        Position {
            x: node.position.x + offset.x,
            y: node.position.y + offset.y,
        }
    }

    pub fn update(&mut self, state: &State, width: i32, height: i32) {
        if !self.changed(state, width, height) {
            return;
        }
        self.key = Some(Self::key(state, width, height));
        self.offsets.clear();
        if state.workspace().graph_arranged {
            return;
        }
        let nodes: Vec<_> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == state.selected_workspace)
            .collect();
        let mut live: Vec<_> = nodes
            .iter()
            .copied()
            .filter(|n| managed(n) && !n.closed_window())
            .collect();
        let mut closed: Vec<_> = nodes
            .iter()
            .copied()
            .filter(|n| n.closed_window())
            .collect();
        for row in [&mut live, &mut closed] {
            row.sort_by(|a, b| {
                a.position
                    .x
                    .total_cmp(&b.position.x)
                    .then_with(|| a.position.y.total_cmp(&b.position.y))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        let start = nodes
            .iter()
            .filter(|n| !managed(n))
            .map(|n| n.position.x + WIDTH + SECTION_GAP)
            .fold(100.0, f64::max);
        let live_width = span(live.len(), LIVE_STEP);
        let live_height = if live.is_empty() { 0.0 } else { HEIGHT + 120.0 };
        let history_y = 80.0
            + live_height
            + if live.is_empty() || closed.is_empty() {
                0.0
            } else {
                SECTION_GAP
            };
        // Choose the grid that leaves the largest readable cards in this viewport.
        // This depends on viewport geometry, never on the current zoom or pan.
        let columns = (1..=closed.len())
            .max_by(|a, b| {
                let scale = |columns: usize| {
                    let w = live_width.max(span(columns, HISTORY_STEP));
                    let h = history_y - 80.0
                        + (closed.len().div_ceil(columns) - 1) as f64 * HISTORY_ROW
                        + CLOSED_HEIGHT;
                    ((width as f64 - 160.0).max(100.0) / w)
                        .min((height as f64 - 160.0).max(100.0) / h)
                        .min(1.2)
                };
                scale(*a).total_cmp(&scale(*b)).then_with(|| a.cmp(b))
            })
            .unwrap_or(0);
        let history_width = span(columns, HISTORY_STEP);
        let section_width = live_width.max(history_width);
        for (index, node) in live.iter().enumerate() {
            self.place(
                node,
                Position {
                    x: start + (section_width - live_width) / 2.0 + index as f64 * LIVE_STEP,
                    y: 80.0,
                },
            );
        }
        for (index, node) in closed.iter().enumerate() {
            self.place(
                node,
                Position {
                    x: start
                        + (section_width - history_width) / 2.0
                        + (index % columns) as f64 * HISTORY_STEP,
                    y: history_y + (index / columns) as f64 * HISTORY_ROW,
                },
            );
        }
    }

    fn place(&mut self, node: &Node, position: Position) {
        self.offsets.insert(
            node.id.clone(),
            Position {
                x: position.x - node.position.x,
                y: position.y - node.position.y,
            },
        );
    }
}

pub const DEFAULT_WRAP_ZOOM: f64 = 0.4;

fn gap() -> f64 {
    crate::config::flow().arrange_gap.unwrap_or(48.)
}

/// Signal flow runs down the graph; data flows across each signal layer.
/// Stable spatial ordering keeps unrelated nodes in their existing relative order.
pub fn arrange(state: &State, width: i32, height: i32, wrap_zoom: f64) -> Vec<(String, Position)> {
    use std::collections::{BTreeMap, BTreeSet};
    let nodes: BTreeMap<_, _> = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
        .map(|n| (n.id.clone(), n))
        .collect();
    if nodes.is_empty() {
        return vec![];
    }
    let mut signals = BTreeSet::new();
    let mut data = BTreeSet::new();
    for edge in &state.connections {
        let (Some(source), Some(_)) = (nodes.get(&edge.source.node), nodes.get(&edge.target.node))
        else {
            continue;
        };
        if edge.source.node == edge.target.node {
            continue;
        }
        let signal = crate::graph::pins(state, source, true)
            .iter()
            .any(|p| p.name == edge.source.pin && p.ty.is_signal());
        if signal { &mut signals } else { &mut data }
            .insert((edge.source.node.clone(), edge.target.node.clone()));
    }
    let spatial = |a: &&String, b: &&String| {
        nodes[*a]
            .position
            .y
            .total_cmp(&nodes[*b].position.y)
            .then_with(|| nodes[*a].position.x.total_cmp(&nodes[*b].position.x))
            .then_with(|| a.cmp(b))
    };
    let mut seeds: Vec<_> = nodes.keys().collect();
    seeds.sort_by(spatial);
    let mut components = HashMap::<String, usize>::new();
    for seed in seeds {
        if components.contains_key(seed) {
            continue;
        }
        let component = components.len();
        let mut pending = vec![seed.clone()];
        while let Some(id) = pending.pop() {
            if components.insert(id.clone(), component).is_some() {
                continue;
            }
            for (a, b) in signals.iter().chain(&data) {
                let neighbour = if a == &id {
                    b
                } else if b == &id {
                    a
                } else {
                    continue;
                };
                if !components.contains_key(neighbour) {
                    pending.push(neighbour.clone());
                }
            }
        }
    }
    let related = |a: &&String, b: &&String| {
        components[*a]
            .cmp(&components[*b])
            .then_with(|| spatial(a, b))
    };
    let mut pending: BTreeSet<_> = nodes.keys().cloned().collect();
    let mut ranks = BTreeMap::<String, usize>::new();
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .filter(|id| !signals.iter().any(|(a, b)| b == *id && pending.contains(a)))
            .min_by(spatial)
            .or_else(|| pending.iter().min_by(spatial))
            .unwrap()
            .clone();
        let rank = signals
            .iter()
            .filter(|(_, b)| b == &ready)
            .filter_map(|(a, _)| ranks.get(a).map(|r| r + 1))
            .max()
            .unwrap_or(0);
        pending.remove(&ready);
        ranks.insert(ready, rank);
    }
    let mut layers = vec![vec![]; ranks.values().copied().max().unwrap_or(0) + 1];
    for rank in 0..layers.len() {
        let mut pending: BTreeSet<_> = nodes
            .keys()
            .filter(|id| ranks[*id] == rank)
            .cloned()
            .collect();
        while !pending.is_empty() {
            let ready = pending
                .iter()
                .filter(|id| !data.iter().any(|(a, b)| b == *id && pending.contains(a)))
                .min_by(related)
                .or_else(|| pending.iter().min_by(related))
                .unwrap()
                .clone();
            pending.remove(&ready);
            layers[rank].push(nodes[&ready]);
        }
    }
    let gap = gap();
    let row_height = |row: &[&Node]| {
        row.iter()
            .map(|n| super::canvas::node_height(n))
            .fold(0., f64::max)
    };
    let max_columns = layers.iter().map(Vec::len).max().unwrap_or(1);
    let min_columns = if wrap_zoom > 0. {
        ((((width as f64 - 160.).max(1.) / wrap_zoom + gap) / (WIDTH + gap)).floor() as usize)
            .max(1)
            .min(max_columns)
    } else {
        max_columns
    };
    let columns = (min_columns..=max_columns)
        .max_by(|a, b| {
            let scale = |columns: usize| {
                let w = columns as f64 * (WIDTH + gap) - gap;
                let h = layers
                    .iter()
                    .flat_map(|l| l.chunks(columns))
                    .map(|r| row_height(r) + gap)
                    .sum::<f64>()
                    - gap;
                ((width as f64 - 160.).max(1.) / w).min((height as f64 - 160.).max(1.) / h)
            };
            scale(*a).total_cmp(&scale(*b)).then_with(|| a.cmp(b))
        })
        .unwrap_or(1);
    let mut placed = HashMap::<String, Position>::new();
    let mut result = vec![];
    let mut y = 80.;
    for row in layers.iter().flat_map(|l| l.chunks(columns)) {
        // Translate a compact row toward its connected predecessors without
        // inserting empty columns between adjacent cards.
        let anchors: Vec<_> = row
            .iter()
            .enumerate()
            .flat_map(|(index, node)| {
                let placed = &placed;
                signals
                    .iter()
                    .filter(move |(_, b)| b == &node.id)
                    .filter_map(move |(a, _)| {
                        placed.get(a).map(|p| p.x - index as f64 * (WIDTH + gap))
                    })
            })
            .collect();
        let x = if anchors.is_empty() {
            100.
        } else {
            (anchors.iter().sum::<f64>() / anchors.len() as f64).max(100.)
        };
        for (index, node) in row.iter().enumerate() {
            let p = Position {
                x: x + index as f64 * (WIDTH + gap),
                y,
            };
            placed.insert(node.id.clone(), p.clone());
            result.push((node.id.clone(), p));
        }
        y += row_height(row) + gap;
    }
    result
}

pub fn align_selection(
    state: &State,
    origins: &HashMap<String, Position>,
    horizontal: bool,
) -> HashMap<String, Position> {
    let mut nodes: Vec<_> = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace && origins.contains_key(&n.id))
        .collect();
    nodes.sort_by(|a, b| {
        let (p, q) = (&origins[&a.id], &origins[&b.id]);
        let (a, b, c, d) = if horizontal {
            (p.x, q.x, p.y, q.y)
        } else {
            (p.y, q.y, p.x, q.x)
        };
        a.total_cmp(&b).then_with(|| c.total_cmp(&d))
    });
    if nodes.is_empty() {
        return HashMap::new();
    }
    let x = origins.values().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let y = origins.values().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let mut cursor = if horizontal { x } else { y };
    nodes
        .into_iter()
        .map(|n| {
            let p = if horizontal {
                Position { x: cursor, y }
            } else {
                Position { x, y: cursor }
            };
            cursor += if horizontal {
                WIDTH
            } else {
                super::canvas::node_height(n)
            } + gap();
            (n.id.clone(), p)
        })
        .collect()
}

fn span(count: usize, step: f64) -> f64 {
    if count == 0 {
        0.0
    } else {
        (count - 1) as f64 * step + WIDTH
    }
}

/// Make room in screen space without changing saved graph positions. Resolve
/// outward in one direction at a time so pushing a row cannot oscillate.
pub fn make_room(
    cards: &mut [(String, super::collapse::Rect)],
    expanded: &[super::collapse::Rect],
) {
    use super::collapse::Rect;
    if expanded.is_empty() {
        return;
    }
    let mut occupied: Vec<_> = expanded.iter().map(|r| r.inset(-20.)).collect();
    cards.sort_by(|a, b| {
        let distance = |r: Rect| {
            expanded
                .iter()
                .map(|e| {
                    (r.x + r.width / 2. - e.x - e.width / 2.)
                        .hypot(r.y + r.height / 2. - e.y - e.height / 2.)
                })
                .fold(f64::INFINITY, f64::min)
        };
        distance(a.1)
            .total_cmp(&distance(b.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    for (_, rect) in cards {
        if occupied.iter().any(|o| rect.overlaps(*o)) {
            let original = *rect;
            *rect = (0..4)
                .map(|direction| {
                    let mut candidate = original;
                    for _ in 0..=occupied.len() {
                        let mut moved = false;
                        for obstacle in &occupied {
                            if candidate.overlaps(*obstacle) {
                                match direction {
                                    0 => candidate.x = obstacle.x - candidate.width,
                                    1 => candidate.x = obstacle.x + obstacle.width,
                                    2 => candidate.y = obstacle.y - candidate.height,
                                    _ => candidate.y = obstacle.y + obstacle.height,
                                }
                                moved = true;
                            }
                        }
                        if !moved {
                            break;
                        }
                    }
                    candidate
                })
                .min_by(|a, b| {
                    (a.x - original.x)
                        .hypot(a.y - original.y)
                        .total_cmp(&(b.x - original.x).hypot(b.y - original.y))
                })
                .unwrap();
        }
        occupied.push(rect.inset(-10.));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeKind;

    #[test]
    fn peeks_clear_all_card_kinds_and_restore_the_saved_layout() {
        use super::super::collapse::Rect;
        let state = scene(8, 8);
        let mut layout = Arrangement::default();
        layout.update(&state, 1200, 800);
        let originals: Vec<_> = state
            .nodes
            .iter()
            .map(|n| {
                (
                    n.id.clone(),
                    Rect {
                        x: layout.position(n).x * 0.7 - 100.,
                        y: layout.position(n).y * 0.7,
                        width: WIDTH * 0.7,
                        height: super::super::canvas::node_height(n) * 0.7,
                    },
                )
            })
            .collect();
        let expanded = Rect {
            x: 200.,
            y: 0.,
            width: 800.,
            height: 600.,
        };
        let mut cards = originals.clone();
        make_room(&mut cards, &[expanded]);
        assert!(cards.iter().any(|(id, rect)| {
            originals
                .iter()
                .any(|(old_id, old)| id == old_id && rect != old)
        }));
        for (i, (_, card)) in cards.iter().enumerate() {
            assert!(!card.overlaps(expanded));
            for (_, other) in &cards[i + 1..] {
                assert!(!card.overlaps(*other));
            }
        }
        let mut restored = originals.clone();
        make_room(&mut restored, &[]);
        assert_eq!(restored, originals);
        let mut again = originals.clone();
        make_room(&mut again, &[expanded]);
        assert_eq!(
            again, cards,
            "the displacement is deterministic and never accumulates"
        );
    }

    fn scene(live: usize, closed: usize) -> State {
        let mut state = State::empty("/tmp".into());
        for index in 0..live + closed {
            let mut node = Node::new(
                &state.selected_workspace,
                NodeKind::Window,
                "/tmp".into(),
                Position {
                    x: 100.0 + index as f64 * 360.0,
                    y: 80.0,
                },
            );
            node.desktop_window = true;
            node.window_id = (index < live).then_some(index as u64);
            state.nodes.push(node);
        }
        state
    }

    #[test]
    fn data_only_links_are_horizontal_compact_and_keep_connected_nodes_adjacent() {
        let mut state = State::empty("/tmp".into());
        for (kind, x) in [
            (NodeKind::Constant, 100.),
            (NodeKind::Window, 2000.),
            (NodeKind::Display, 4000.),
        ] {
            state.nodes.push(Node::new(
                &state.selected_workspace,
                kind,
                "/tmp".into(),
                Position { x, y: 80. },
            ));
        }
        state.connections.push(crate::graph::Connection {
            source: crate::graph::Endpoint {
                node: state.nodes[0].id.clone(),
                pin: "value".into(),
            },
            target: crate::graph::Endpoint {
                node: state.nodes[2].id.clone(),
                pin: "value".into(),
            },
        });
        let positions: HashMap<_, _> = arrange(&state, 1800, 1000, 0.).into_iter().collect();
        let (a, b, c) = (
            &positions[&state.nodes[0].id],
            &positions[&state.nodes[1].id],
            &positions[&state.nodes[2].id],
        );
        assert_eq!(a.y, c.y);
        assert_eq!(a.y, b.y);
        assert_eq!(c.x - a.x, WIDTH + gap());
        assert!(b.x > c.x);
    }

    #[test]
    fn directional_alignment_uses_selected_positions_and_variable_card_heights() {
        let mut state = scene(2, 0);
        state.nodes.push(Node::new(
            &state.selected_workspace,
            NodeKind::Shell,
            "/tmp".into(),
            Position { x: 500., y: 1000. },
        ));
        let origins: HashMap<_, _> = state.nodes[..2]
            .iter()
            .map(|n| (n.id.clone(), n.position.clone()))
            .collect();
        let vertical = align_selection(&state, &origins, false);
        assert_eq!(vertical.len(), 2);
        let (a, b) = (&vertical[&state.nodes[0].id], &vertical[&state.nodes[1].id]);
        assert_eq!(a.x, b.x);
        assert_eq!(
            b.y - a.y,
            super::super::canvas::node_height(&state.nodes[0]) + gap()
        );
        let horizontal = align_selection(&state, &vertical, true);
        let (a, b) = (
            &horizontal[&state.nodes[0].id],
            &horizontal[&state.nodes[1].id],
        );
        assert_eq!(a.y, b.y);
        assert_eq!(b.x - a.x, WIDTH + gap());
    }

    #[test]
    fn arranging_persists_and_replaces_the_live_window_offsets() {
        let mut state = scene(2, 0);
        let source = state.nodes[0].id.clone();
        state.connections.push(crate::graph::Connection {
            source: crate::graph::Endpoint {
                node: source,
                pin: "on_exit".into(),
            },
            target: crate::graph::Endpoint {
                node: state.nodes[1].id.clone(),
                pin: "in".into(),
            },
        });
        let mut layout = Arrangement::default();
        layout.update(&state, 1200, 800);
        let positions = arrange(&state, 1200, 800, DEFAULT_WRAP_ZOOM);
        state.workspaces[0].graph_arranged = true;
        for (id, p) in positions {
            state.node_mut(&id).unwrap().position = p;
        }
        assert!(layout.changed(&state, 1200, 800));
        layout.update(&state, 1200, 800);
        for node in &state.nodes {
            let p = layout.position(node);
            assert_eq!((p.x, p.y), (node.position.x, node.position.y));
        }
        assert!(
            state.nodes[1].position.y
                > state.nodes[0].position.y + super::super::canvas::node_height(&state.nodes[0])
        );
        let serialized = serde_json::to_string(&state).unwrap();
        let restored: State = serde_json::from_str(&serialized).unwrap();
        let mut layout = Arrangement::default();
        layout.update(&restored, 1200, 800);
        assert_eq!(
            layout.position(&restored.nodes[1]).y,
            state.nodes[1].position.y
        );
    }

    #[test]
    fn arranging_zooms_out_before_wrapping_and_respects_the_threshold() {
        let state = scene(6, 0);
        for (width, height) in [(1200, 800), (1828, 1095)] {
            let positions = arrange(&state, width, height, DEFAULT_WRAP_ZOOM);
            assert!(
                positions.iter().all(|(_, p)| p.y == 80.),
                "six windows must stay together on a desktop-sized viewport"
            );
        }
        let earlier_wrap = arrange(&state, 1200, 800, 1.);
        assert!(earlier_wrap.iter().any(|(_, p)| p.y > 80.));
        let large = scene(40, 0);
        let unwrapped = arrange(&large, 1200, 800, 0.);
        assert!(unwrapped.iter().all(|(_, p)| p.y == 80.));
        let wrapped = arrange(&large, 1828, 1095, DEFAULT_WRAP_ZOOM);
        assert!(wrapped.iter().any(|(_, p)| p.y > 80.));
        assert!(wrapped.iter().filter(|(_, p)| p.y == 80.).count() >= 6);
        assert!(arrange(&scene(0, 0), 1200, 800, DEFAULT_WRAP_ZOOM).is_empty());
    }

    #[test]
    fn arranging_many_windows_wraps_to_the_viewport_and_is_idempotent() {
        for (width, height) in [(1200, 800), (800, 1200), (100, 100)] {
            let mut state = scene(40, 0);
            let positions = arrange(&state, width, height, DEFAULT_WRAP_ZOOM);
            assert_eq!(positions.len(), 40);
            assert!(positions.iter().any(|(_, p)| p.y > 80.));
            for (id, p) in &positions {
                assert!(p.x.is_finite() && p.y.is_finite());
                state.node_mut(id).unwrap().position = p.clone();
            }
            let coordinates = |positions: Vec<(String, Position)>| {
                positions
                    .into_iter()
                    .map(|(id, p)| (id, p.x, p.y))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                coordinates(positions.clone()),
                coordinates(arrange(&state, width, height, DEFAULT_WRAP_ZOOM))
            );
            let bounds = positions.iter().fold((0_f64, 0_f64), |(w, h), (_, p)| {
                (w.max(p.x - 100. + WIDTH), h.max(p.y - 80. + HEIGHT + 120.))
            });
            if width > height {
                assert!(bounds.0 > bounds.1);
            } else if width < height {
                assert!(bounds.0 < bounds.1);
            }
        }
    }

    #[test]
    fn wrapped_layers_clear_mixed_card_heights_and_preserve_dependencies() {
        let mut state = scene(12, 3);
        let target = Node::new(
            &state.selected_workspace,
            NodeKind::Human,
            "/tmp".into(),
            Position::default(),
        );
        for node in &state.nodes {
            state.connections.push(crate::graph::Connection {
                source: crate::graph::Endpoint {
                    node: node.id.clone(),
                    pin: "on_exit".into(),
                },
                target: crate::graph::Endpoint {
                    node: target.id.clone(),
                    pin: "in".into(),
                },
            });
        }
        let target_id = target.id.clone();
        state.nodes.push(target);
        let positions: HashMap<_, _> = arrange(&state, 1200, 800, DEFAULT_WRAP_ZOOM)
            .into_iter()
            .collect();
        for (i, a) in state.nodes.iter().enumerate() {
            let p = &positions[&a.id];
            for b in state.nodes.iter().skip(i + 1) {
                let q = &positions[&b.id];
                assert!(
                    p.x + WIDTH <= q.x
                        || q.x + WIDTH <= p.x
                        || p.y + super::super::canvas::node_height(a) <= q.y
                        || q.y + super::super::canvas::node_height(b) <= p.y
                );
            }
            if a.id != target_id {
                assert!(p.y + super::super::canvas::node_height(a) < positions[&target_id].y);
            }
        }
    }

    #[test]
    fn live_windows_stay_horizontal_and_history_wraps_without_overlaps() {
        let state = scene(3, 20);
        let mut layout = Arrangement::default();
        layout.update(&state, 1828, 1095);
        let positions: Vec<_> = state.nodes.iter().map(|n| layout.position(n)).collect();
        assert!(
            positions[..3]
                .windows(2)
                .all(|p| p[0].y == p[1].y && p[0].x + WIDTH < p[1].x)
        );
        assert!(
            positions[3..]
                .iter()
                .all(|p| p.y > positions[0].y + HEIGHT + 120.0)
        );
        assert!(positions[3..].iter().any(|p| p.y > positions[3].y));
        for (i, a) in positions.iter().enumerate() {
            for (j, b) in positions.iter().enumerate().skip(i + 1) {
                let h = |index| {
                    if index < 3 {
                        HEIGHT + 120.0
                    } else {
                        CLOSED_HEIGHT
                    }
                };
                assert!(
                    a.x + WIDTH <= b.x
                        || b.x + WIDTH <= a.x
                        || a.y + h(i) <= b.y
                        || b.y + h(j) <= a.y
                );
            }
        }
    }

    #[test]
    fn history_only_and_small_viewports_have_finite_positions() {
        for (live, closed) in [(0, 0), (0, 1), (0, 20), (2, 0), (2, 20)] {
            let state = scene(live, closed);
            let mut layout = Arrangement::default();
            layout.update(&state, 100, 100);
            assert!(state.nodes.iter().all(|n| {
                let p = layout.position(n);
                p.x.is_finite() && p.y.is_finite()
            }));
        }
    }

    #[test]
    fn closing_and_reopening_reflows_but_moving_does_not_reset_manual_placement() {
        let mut state = scene(2, 7);
        let mut layout = Arrangement::default();
        layout.update(&state, 1828, 1095);
        let before = layout.position(&state.nodes[0]);
        state.nodes[0].position.y += 40.0;
        layout.update(&state, 1828, 1095);
        assert_eq!(layout.position(&state.nodes[0]).y, before.y + 40.0);
        state.nodes[0].window_id = None;
        assert!(layout.changed(&state, 1828, 1095));
        layout.update(&state, 1828, 1095);
        assert!(layout.position(&state.nodes[0]).y > layout.position(&state.nodes[1]).y);
        state.nodes[0].window_id = Some(50);
        layout.update(&state, 1828, 1095);
        assert_eq!(
            layout.position(&state.nodes[0]).y,
            layout.position(&state.nodes[1]).y
        );
    }
}
