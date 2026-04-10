// src/server.rs

use crate::database::Database;
use crate::utils::xor_in_place;

pub struct QueryServer<'a> {
    db: &'a Database, 
}

impl<'a> QueryServer<'a> {
    pub fn new(db: &'a Database) -> Self {
        QueryServer { db }
    }

    /// Respond to client's Query
    /// Takes a list of global indices to query, returns merged Parity (size B bytes)
    pub fn answer_query(&self, query_indices: &[usize]) -> Vec<u8> {
        // Create an initial block of zeros, same size as record size B
        let mut result_parity = vec![0u8; self.db.record_size];

        for &idx in query_indices {
            if let Some(record) = self.db.get_record(idx) {
                // XOR the scanned data block into result_parity
                xor_in_place(&mut result_parity, record);
            }
        }

        result_parity
    }
}