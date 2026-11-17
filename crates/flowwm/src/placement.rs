//! Place new cards against occupied workspace bounds rather than node counts.
use crate::model::{Node, NodeKind, Position, State};

pub const WIDTH: f64 = 292.;
pub const HEIGHT: f64 = 154.;
pub const CLOSED_HEIGHT: f64 = 78.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Card,
    Timer,
    SessionAction,
    Routing,
    Environment,
    List,
    Value,
    Display,
    Alert,
    Decision,
}

pub fn surface(node: &Node) -> Surface {
    use Surface::*;
    if node.window_id.is_some() || node.kind == NodeKind::Window || node.desktop_window {
        return Card;
    }
    match node.flow.node_type.as_str() {
        "local.timer" => return Timer,
        id if flow_builtins::session_actions::name(id).is_some() => return SessionAction,
        "local.join" | "local.on_all_true" | "local.on_all_fired" | "local.broadcast"
        | "local.receive" => return Routing,
        "local.set_env" | "local.unset_env" | "local.empty_env" | "local.bake_args_into_env" => {
            return Environment;
        }
        id if id.strip_prefix("local.").is_some_and(|id| {
            flow_builtins::lists::NODES
                .iter()
                .any(|(name, _, _)| id == *name)
        }) =>
        {
            return List;
        }
        _ => (),
    }
    match node.kind {
        NodeKind::Constant => Value,
        NodeKind::Display => Display,
        NodeKind::Alert => Alert,
        NodeKind::Gate | NodeKind::Human => Decision,
        NodeKind::Conditional | NodeKind::BreakSignal | NodeKind::MergeSignal => Routing,
        _ => Card,
    }
}

pub fn node_width(node: &Node) -> f64 {
    if node.kind == NodeKind::Window || node.window_id.is_some() || node.desktop_window {
        return WIDTH;
    }
    if let Some(renderer) = &node.renderer {
        return renderer.size[0];
    }
    match surface(node) {
        Surface::SessionAction => 148.,
        Surface::Timer => 224.,
        Surface::Routing => 172.,
        Surface::Value | Surface::List => 208.,
        Surface::Environment | Surface::Alert => 224.,
        Surface::Display => 260.,
        _ => WIDTH,
    }
}

pub fn node_height(node: &Node) -> f64 {
    if node.closed_window() {
        CLOSED_HEIGHT
    } else if node.kind == NodeKind::Window || node.window_id.is_some() {
        HEIGHT + 120.
    } else if let Some(renderer) = &node.renderer {
        renderer.size[1]
    } else if node.kind == NodeKind::Vox {
        252.
    } else {
        match surface(node) {
            Surface::SessionAction => 148.,
            Surface::Timer => 168.,
            Surface::Routing => 140.,
            Surface::Value => 126.,
            Surface::Display => 160.,
            Surface::Alert => 148.,
            Surface::Decision => 152.,
            _ => 126.,
        }
    }
}

pub fn gap() -> f64 {
    crate::config::flow().arrange_gap.unwrap_or(48.)
}

struct Bounds {
    position: Position,
    width: f64,
    height: f64,
}

pub struct Space {
    occupied: Vec<Bounds>,
    gap: f64,
}

impl Space {
    pub fn new(
        state: &State,
        workspace: &str,
        mut position: impl FnMut(&Node) -> Position,
    ) -> Self {
        let occupied = state
            .nodes
            .iter()
            .filter(|node| node.workspace == workspace)
            .map(|node| Bounds {
                position: position(node),
                width: node_width(node),
                height: node_height(node),
            })
            .chain(
                state
                    .workspaces
                    .iter()
                    .find(|row| row.id == workspace)
                    .into_iter()
                    .flat_map(|row| &row.canvas)
                    .filter(|item| matches!(item.content, crate::annotations::Content::Text { .. }))
                    .map(|item| Bounds {
                        position: item.position.clone(),
                        width: item.width,
                        height: item.height,
                    }),
            )
            .collect();
        Self {
            occupied,
            gap: gap(),
        }
    }

    fn overlaps(&self, position: &Position, width: f64, height: f64, other: &Bounds) -> bool {
        position.x < other.position.x + other.width + self.gap
            && position.x + width + self.gap > other.position.x
            && position.y < other.position.y + other.height + self.gap
            && position.y + height + self.gap > other.position.y
    }

    /// Fill the first free space in the graph's top row, preserving window order.
    pub fn append(&self, width: f64, height: f64) -> Position {
        let mut position = Position {
            x: self
                .occupied
                .iter()
                .map(|b| b.position.x)
                .reduce(f64::min)
                .unwrap_or(100.),
            y: self
                .occupied
                .iter()
                .map(|b| b.position.y)
                .reduce(f64::min)
                .unwrap_or(80.),
        };
        let mut occupied: Vec<_> = self.occupied.iter().collect();
        occupied.sort_by(|a, b| a.position.x.total_cmp(&b.position.x));
        for other in occupied {
            if self.overlaps(&position, width, height, other) {
                position.x = other.position.x + other.width + self.gap;
            }
        }
        position
    }

    /// Try free space on every side of nearby cards instead of drifting right.
    pub fn near(&self, preferred: Position, width: f64, height: f64) -> Position {
        let free = |position: &Position| {
            self.occupied
                .iter()
                .all(|other| !self.overlaps(position, width, height, other))
        };
        if free(&preferred) {
            return preferred;
        }
        let distance = |p: &Position| (p.x - preferred.x).hypot(p.y - preferred.y);
        self.occupied
            .iter()
            .flat_map(|other| {
                [
                    Position {
                        x: other.position.x + other.width + self.gap,
                        y: preferred.y,
                    },
                    Position {
                        x: preferred.x,
                        y: other.position.y + other.height + self.gap,
                    },
                    Position {
                        x: other.position.x - width - self.gap,
                        y: preferred.y,
                    },
                    Position {
                        x: preferred.x,
                        y: other.position.y - height - self.gap,
                    },
                ]
            })
            .filter(free)
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
            .expect("the outermost card always has free space beside it")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crowded_rows_offer_nearby_space_instead_of_pushing_creation_to_the_end() {
        let mut state = State::empty("/tmp".into());
        for index in 0..20 {
            state.nodes.push(Node::new(
                &state.selected_workspace,
                NodeKind::Window,
                "/tmp".into(),
                Position {
                    x: index as f64 * (WIDTH + gap()),
                    y: 0.,
                },
            ));
        }
        let space = Space::new(&state, &state.selected_workspace, |n| n.position.clone());
        let preferred = Position {
            x: 2. * (WIDTH + gap()),
            y: 120.,
        };
        let position = space.near(preferred.clone(), WIDTH, 252.);
        assert_eq!(position.x, preferred.x);
        assert_eq!(position.y, HEIGHT + 120. + gap());
    }

    #[test]
    fn placement_uses_both_card_sizes_text_bounds_and_only_the_target_workspace() {
        let mut state = State::empty("/tmp".into());
        state.nodes.push(Node::new(
            "another-workspace",
            NodeKind::Window,
            "/tmp".into(),
            Position {
                x: -5000.,
                y: -5000.,
            },
        ));
        let mut text = crate::annotations::Annotation::text(Position { x: 100., y: 80. });
        text.width = 900.;
        text.height = 600.;
        state.workspaces[0].canvas.push(text);
        let space = Space::new(&state, &state.selected_workspace, |n| n.position.clone());
        let position = space.append(WIDTH, 252.);
        assert_eq!((position.x, position.y), (1000. + gap(), 80.));
        let preferred = Position {
            x: 100.,
            y: 80. - 126. - gap(),
        };
        let position = space.near(preferred.clone(), WIDTH, 252.);
        assert!(position.x != preferred.x || position.y + 252. + gap() <= 80.);
        let free = Position { x: 2400., y: 1700. };
        let position = space.near(free.clone(), WIDTH, 252.);
        assert_eq!((position.x, position.y), (free.x, free.y));
    }
}
