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

/// Layer a directed graph with deterministic cycle breaking and stable ordering.
pub fn arrange(state: &State) -> Vec<(String, Position)> {
    use std::collections::{BTreeMap, BTreeSet};
    let nodes: BTreeMap<_, _> = state
        .nodes
        .iter()
        .filter(|n| n.workspace == state.selected_workspace)
        .map(|n| (n.id.clone(), n))
        .collect();
    let mut edges = BTreeSet::new();
    for node in nodes.values() {
        for source in &node.dependencies {
            edges.insert((source.clone(), node.id.clone()));
        }
    }
    for e in &state.connections {
        edges.insert((e.source.node.clone(), e.target.node.clone()));
    }
    for w in &state.watchers {
        edges.insert((w.source.clone(), w.target.clone()));
    }
    edges.retain(|(a, b)| nodes.contains_key(a) && nodes.contains_key(b) && a != b);
    let mut pending: BTreeSet<_> = nodes.keys().cloned().collect();
    let mut ranks = BTreeMap::<String, usize>::new();
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .find(|id| !edges.iter().any(|(a, b)| b == *id && pending.contains(a)))
            .cloned()
            .unwrap_or_else(|| pending.first().unwrap().clone());
        let rank = edges
            .iter()
            .filter(|(_, b)| b == &ready)
            .filter_map(|(a, _)| ranks.get(a).map(|r| r + 1))
            .max()
            .unwrap_or(0);
        pending.remove(&ready);
        ranks.insert(ready, rank);
    }
    let mut result = vec![];
    let mut y = 80.0;
    for rank in 0..=ranks.values().copied().max().unwrap_or(0) {
        let mut layer: Vec<_> = nodes
            .values()
            .copied()
            .filter(|n| ranks[&n.id] == rank)
            .collect();
        layer.sort_by(|a, b| {
            a.position
                .x
                .total_cmp(&b.position.x)
                .then_with(|| a.id.cmp(&b.id))
        });
        let height = layer
            .iter()
            .map(|n| super::canvas::node_height(n))
            .fold(0., f64::max);
        for (index, node) in layer.iter().enumerate() {
            result.push((
                node.id.clone(),
                Position {
                    x: 100. + index as f64 * (WIDTH + 100.),
                    y,
                },
            ));
        }
        y += height + 110.;
    }
    result
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
    fn arranging_persists_and_replaces_the_live_window_offsets() {
        let mut state = scene(2, 0);
        let source = state.nodes[0].id.clone();
        state.nodes[1].dependencies.push(source);
        let mut layout = Arrangement::default();
        layout.update(&state, 1200, 800);
        let positions = arrange(&state);
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
