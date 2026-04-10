// src/client.rs

use std::collections::HashMap;

pub type HintId = usize;
pub type DatabaseIndex = usize;

pub struct ClientTable {
    /// Mapping: Database Index -> List of Hint IDs covering it
    /// This allows O(1) lookup to find which hint to use for a query.
    pub table: HashMap<DatabaseIndex, Vec<HintId>>,
}

impl ClientTable {
    pub fn new() -> Self {
        ClientTable {
            table: HashMap::new(),
        }
    }

    /// Register a Hint into the table using its expanded global indices.
    /// This is the O(N)_p offline setup cost.
    pub fn register_hint(&mut self, id: HintId, indices: Vec<DatabaseIndex>) {
        for idx in indices {
            self.table.entry(idx).or_insert_with(Vec::new).push(id);
        }
    }

    /// Find a Hint that covers the target index.
    /// In the real protocol, you would pick one and move it/refresh it.
    pub fn find_hint_for(&self, target: DatabaseIndex) -> Option<HintId> {
        self.table.get(&target).and_then(|hints| hints.first().cloned())
    }

    /// Logical index replacement: Update the table after a query.
    /// Moves a Hint from covering index 'z' to covering index 'x_star'.
    pub fn replace_index(&mut self, hint_id: HintId, old_idx: DatabaseIndex, new_idx: DatabaseIndex) {
        // 1. Remove HintID from the old index's list
        if let Some(hints) = self.table.get_mut(&old_idx) {
            hints.retain(|&id| id != hint_id);
        }

        // 2. Add HintID to the new index's list
        self.table.entry(new_idx).or_insert_with(Vec::new).push(hint_id);
    }
}