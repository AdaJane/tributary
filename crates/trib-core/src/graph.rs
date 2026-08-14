//! The signal graph as explicit data — always derived from `MixerState`,
//! never stored, so the two can't drift.

use crate::bus::BusKind;
use crate::id::{BusId, FxId, StripId};
use crate::mix::MixerState;
use crate::strip::{RouteTarget, SendTap};

/// Graph-internal index. Never persisted, never on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Strip(StripId),
    Bus(BusId),
    Fx(FxId),
    Master,
}

impl NodeKind {
    /// The v1 cycle rule: edges must be stage-increasing, so cycles are
    /// impossible by construction. `topo_order` still verifies — when
    /// routing loosens later, the check becomes the real gate.
    pub fn stage(self, buses: &[(BusId, BusKind)]) -> u8 {
        match self {
            NodeKind::Strip(_) => 1,
            NodeKind::Bus(id) => match buses.iter().find(|(b, _)| *b == id) {
                Some((_, BusKind::Group)) => 2,
                Some((_, BusKind::Aux)) => 3,
                None => 2,
            },
            NodeKind::Fx(_) => 4,
            NodeKind::Master => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// The node's main (post-fader) signal.
    Main,
    /// An aux send tapped pre- or post-fader.
    Send { tap: SendTap },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignalGraph {
    /// Index in this vec IS the `NodeId`.
    pub nodes: Vec<NodeKind>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("routing would create a cycle")]
pub struct CycleError;

/// Derive the graph from the document. Dangling references (a send to a
/// missing bus, an FX fed by a missing bus) are simply not edges: the
/// reducer refuses to create them, and a hand-edited manifest degrades to
/// silence on that path rather than a dead daemon.
pub fn derive_graph(state: &MixerState) -> SignalGraph {
    let mut nodes = Vec::new();
    let index = |kind: NodeKind, nodes: &mut Vec<NodeKind>| -> NodeId {
        nodes.push(kind);
        NodeId((nodes.len() - 1) as u32)
    };

    let strip_nodes: Vec<(StripId, NodeId)> = state
        .strips
        .iter()
        .map(|s| (s.id, index(NodeKind::Strip(s.id), &mut nodes)))
        .collect();
    let bus_nodes: Vec<(BusId, NodeId)> = state
        .buses
        .iter()
        .map(|b| (b.id, index(NodeKind::Bus(b.id), &mut nodes)))
        .collect();
    let fx_nodes: Vec<(FxId, NodeId)> = state
        .fx
        .iter()
        .map(|f| (f.id, index(NodeKind::Fx(f.id), &mut nodes)))
        .collect();
    let master = index(NodeKind::Master, &mut nodes);

    let bus_node = |id: BusId| bus_nodes.iter().find(|(b, _)| *b == id).map(|(_, n)| *n);

    let mut edges = Vec::new();
    for strip in &state.strips {
        let (_, from) = strip_nodes
            .iter()
            .find(|(id, _)| *id == strip.id)
            .expect("strip node exists");
        let to = match strip.route_to {
            RouteTarget::Master => Some(master),
            RouteTarget::Bus { id } => bus_node(id),
        };
        if let Some(to) = to {
            edges.push(Edge {
                from: *from,
                to,
                kind: EdgeKind::Main,
            });
        }
        for send in &strip.sends {
            if let Some(to) = bus_node(send.dest) {
                edges.push(Edge {
                    from: *from,
                    to,
                    kind: EdgeKind::Send { tap: send.tap },
                });
            }
        }
    }
    for bus in &state.buses {
        let from = bus_node(bus.id).expect("bus node exists");
        match bus.kind {
            // Groups mix into the master.
            BusKind::Group => edges.push(Edge {
                from,
                to: master,
                kind: EdgeKind::Main,
            }),
            // Aux buses feed FX units (via the fx.input reference below).
            BusKind::Aux => {}
        }
    }
    for fx in &state.fx {
        let (_, fx_node) = fx_nodes
            .iter()
            .find(|(id, _)| *id == fx.id)
            .expect("fx node exists");
        if let Some(from) = bus_node(fx.input) {
            edges.push(Edge {
                from,
                to: *fx_node,
                kind: EdgeKind::Main,
            });
        }
        // FX returns to the master in v1.
        edges.push(Edge {
            from: *fx_node,
            to: master,
            kind: EdgeKind::Main,
        });
    }

    SignalGraph { nodes, edges }
}

/// Kahn's algorithm; deterministic (stable by node index) so the compiled
/// evaluation order never depends on hash order.
pub fn topo_order(graph: &SignalGraph) -> Result<Vec<NodeId>, CycleError> {
    let n = graph.nodes.len();
    let mut in_degree = vec![0usize; n];
    for edge in &graph.edges {
        in_degree[edge.to.0 as usize] += 1;
    }
    let mut order = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    while let Some(node) = ready.pop() {
        order.push(NodeId(node as u32));
        for edge in &graph.edges {
            if edge.from.0 as usize == node {
                let to = edge.to.0 as usize;
                in_degree[to] -= 1;
                if in_degree[to] == 0 {
                    ready.push(to);
                }
            }
        }
        // Keep determinism: smallest index first.
        ready.sort_unstable_by(|a, b| b.cmp(a));
    }
    if order.len() == n {
        Ok(order)
    } else {
        Err(CycleError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::BusState;
    use crate::fx::{FxParams, FxState};
    use crate::mix::MasterState;
    use crate::strip::{SendState, StripState};

    fn console() -> MixerState {
        let mut vocal = StripState::new(StripId(0), "Vocal".into());
        vocal.sends.push(SendState {
            dest: BusId(1),
            level_db: -12.0,
            tap: SendTap::PostFader,
        });
        let mut gtr = StripState::new(StripId(1), "Gtr".into());
        gtr.route_to = RouteTarget::Bus { id: BusId(0) };
        MixerState {
            strips: vec![vocal, gtr],
            buses: vec![
                BusState::new(BusId(0), BusKind::Group, "Band".into()),
                BusState::new(BusId(1), BusKind::Aux, "FX Send".into()),
            ],
            fx: vec![FxState {
                id: FxId(0),
                name: "Verb".into(),
                input: BusId(1),
                params: FxParams::default_reverb(),
                return_level_db: -6.0,
            }],
            // The signal graph knows nothing about instruments: they are an
            // input source, so they enter the graph through a strip's patch
            // like any device, never as a node of their own.
            instruments: Vec::new(),
            master: MasterState::default(),
        }
    }

    #[test]
    fn derive_covers_routes_sends_and_returns() {
        let graph = derive_graph(&console());
        // 2 strips + 2 buses + 1 fx + master.
        assert_eq!(graph.nodes.len(), 6);
        // vocal→master, vocal→aux (send), gtr→group, group→master,
        // aux→fx, fx→master.
        assert_eq!(graph.edges.len(), 6);
    }

    #[test]
    fn topo_order_puts_sources_before_sinks() {
        let graph = derive_graph(&console());
        let order = topo_order(&graph).unwrap();
        let position = |kind: NodeKind| {
            order
                .iter()
                .position(|id| graph.nodes[id.0 as usize] == kind)
                .unwrap()
        };
        assert!(position(NodeKind::Strip(StripId(0))) < position(NodeKind::Master));
        assert!(position(NodeKind::Bus(BusId(1))) < position(NodeKind::Fx(FxId(0))));
        assert!(position(NodeKind::Fx(FxId(0))) < position(NodeKind::Master));
    }

    #[test]
    fn topo_order_is_deterministic() {
        let graph = derive_graph(&console());
        assert_eq!(topo_order(&graph).unwrap(), topo_order(&graph).unwrap());
    }

    #[test]
    fn a_hand_made_cycle_is_detected() {
        let graph = SignalGraph {
            nodes: vec![NodeKind::Bus(BusId(0)), NodeKind::Bus(BusId(1))],
            edges: vec![
                Edge {
                    from: NodeId(0),
                    to: NodeId(1),
                    kind: EdgeKind::Main,
                },
                Edge {
                    from: NodeId(1),
                    to: NodeId(0),
                    kind: EdgeKind::Main,
                },
            ],
        };
        assert_eq!(topo_order(&graph), Err(CycleError));
    }

    #[test]
    fn a_send_to_a_missing_bus_is_dropped_not_fatal() {
        let mut state = console();
        state.strips[0].sends[0].dest = BusId(99);
        let graph = derive_graph(&state);
        assert_eq!(graph.edges.len(), 5, "the dangling send edge vanished");
        assert!(topo_order(&graph).is_ok());
    }
}
