/// Nodelocking: fix a player's strategy at specific decision nodes.
///
/// The other player's strategy is solved freely (best response / equilibrium against locks).
/// This is the core mechanism for "what would GTO do if villain plays X here?" analysis.
///
/// A NodeLock specifies:
/// - The action sequence identifying the decision node (empty vec = root)
/// - The player being locked (OOP or IP)
/// - Per-action frequencies (for the locked player's strategy at this node)

use crate::game::{Action, Player};
use rustc_hash::FxHashMap;

/// A locked strategy at a specific decision node.
#[derive(Clone, Debug)]
pub struct NodeLock {
    /// The player whose strategy is fixed at this node.
    pub player: Player,
    /// Per-action frequencies. frequencies[i] = probability of actions[i].
    /// Must sum to 1.0 (will be normalized if not).
    /// Length must match the number of actions at the node.
    pub frequencies: Vec<f32>,
}

/// Collection of node locks, keyed by action sequence.
/// Empty vec = root node.
#[derive(Clone, Debug, Default)]
pub struct NodeLocks {
    /// action_seq → NodeLock
    pub locks: FxHashMap<Vec<Action>, NodeLock>,
    /// action_seq → tree node ID (populated after tree is built)
    node_to_id: FxHashMap<Vec<Action>, u32>,
    /// tree node ID → NodeLock (fast lookup during traversal)
    pub id_to_lock: FxHashMap<u32, NodeLock>,
}

impl NodeLocks {
    pub fn new() -> Self {
        NodeLocks {
            locks: FxHashMap::default(),
            node_to_id: FxHashMap::default(),
            id_to_lock: FxHashMap::default(),
        }
    }

    /// Add a lock at the given action sequence.
    /// This is called BEFORE tree building; id_to_lock is populated after build.
    pub fn add_lock(&mut self, action_seq: Vec<Action>, lock: NodeLock) {
        self.locks.insert(action_seq, lock);
    }

    /// Number of locked nodes.
    pub fn len(&self) -> usize {
        self.locks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.locks.is_empty()
    }

    /// After building the tree, resolve action_seq → id_to_lock for fast traversal lookup.
    pub fn resolve_node_ids(&mut self, node_index: &FxHashMap<Vec<Action>, u32>) {
        self.id_to_lock.clear();
        self.node_to_id.clear();
        for (action_seq, lock) in &self.locks {
            if let Some(&node_id) = node_index.get(action_seq) {
                self.node_to_id.insert(action_seq.clone(), node_id);
                self.id_to_lock.insert(node_id, lock.clone());
            }
        }
    }

    /// Check if a tree node ID is locked, and return the lock.
    #[inline]
    pub fn get_lock_for_node(&self, node_id: u32) -> Option<&NodeLock> {
        self.id_to_lock.get(&node_id)
    }

    /// Iterate over all locks (for export/public API).
    pub fn iter(&self) -> impl Iterator<Item = (&Vec<Action>, &NodeLock)> {
        self.locks.iter()
    }
}

/// Result of solving with node locks: EV penalty and exploitability-against-locks.
pub struct LockSolveInfo {
    /// The locked player's EV under the locked strategy.
    pub locked_ev: f32,
    /// The locked player's EV under their best response (if they were free).
    /// This gives the EV penalty of being locked.
    pub locked_br_ev: f32,
    /// The unlocked player's EV.
    pub unlocked_ev: f32,
    /// Exploitability of the resulting strategy (should be near 0 for the free player,
    /// may be non-zero for the locked player due to suboptimal locks).
    pub exploitability_pct: f32,
}