// src/server.rs

use crate::crypto::{derive_client_key, derive_hint_seed, expand_hint};
use crate::database::Database;
use crate::utils::xor_in_place;
use serde::Serialize;

pub type HintId = usize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ParityPair {
    pub rho_s: Vec<u8>,
    pub rho_v: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ServerHintPacket {
    pub hint_id: HintId,
    pub parities: ParityPair,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BootstrapRequest {
    pub client_id: String,
    pub total_hints: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BootstrapResponse {
    pub usk: [u8; 32],
    pub hint_packets: Vec<ServerHintPacket>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RefreshRequest {
    pub client_id: String,
    pub next_hint_id: HintId,
    pub batch_size: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RefreshResponse {
    pub hint_packets: Vec<ServerHintPacket>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PatchedQueryRequest {
    pub offsets: Vec<usize>,
    pub masks: Vec<bool>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PatchedQueryResponse {
    pub parities: ParityPair,
}

pub struct HintServer<'a> {
    db: &'a Database,
    master_secret: [u8; 32],
}

pub struct QueryServer<'a> {
    db: &'a Database,
}

impl<'a> HintServer<'a> {
    pub fn new(db: &'a Database, master_secret: [u8; 32]) -> Self {
        Self { db, master_secret }
    }

    pub fn handle_bootstrap(&self, request: BootstrapRequest) -> BootstrapResponse {
        let usk = derive_client_key(&self.master_secret, &request.client_id);
        let hint_packets = self.build_hint_packets(&usk, 0, request.total_hints);

        BootstrapResponse { usk, hint_packets }
    }

    pub fn handle_refresh(&self, request: RefreshRequest) -> RefreshResponse {
        let usk = derive_client_key(&self.master_secret, &request.client_id);
        let hint_packets = self.build_hint_packets(&usk, request.next_hint_id, request.batch_size);
        RefreshResponse { hint_packets }
    }

    fn build_hint_packets(
        &self,
        usk: &[u8; 32],
        start_hint_id: HintId,
        count: usize,
    ) -> Vec<ServerHintPacket> {
        (start_hint_id..start_hint_id + count)
            .map(|hint_id| {
                let seed = derive_hint_seed(usk, hint_id);
                let expansion = expand_hint(
                    seed,
                    self.db.partition_count,
                    self.db.records_per_partition,
                );
                let parities = self.compute_hint_parities(&expansion.offsets, &expansion.masks);

                ServerHintPacket { hint_id, parities }
            })
            .collect()
    }

    fn compute_hint_parities(&self, offsets: &[usize], masks: &[bool]) -> ParityPair {
        let mut rho_s = vec![0u8; self.db.record_size];
        let mut rho_v = vec![0u8; self.db.record_size];

        for partition_idx in 0..self.db.partition_count {
            let record = &self.db.data[partition_idx][offsets[partition_idx]];
            xor_in_place(&mut rho_s, record);

            if masks[partition_idx] {
                xor_in_place(&mut rho_v, record);
            }
        }

        ParityPair { rho_s, rho_v }
    }
}

impl<'a> QueryServer<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Compute the patched server response (T_s, T_v) described in the paper.
    pub fn handle_query(&self, request: PatchedQueryRequest) -> PatchedQueryResponse {
        let offsets = request.offsets;
        let masks = request.masks;
        let mut t_s = vec![0u8; self.db.record_size];
        let mut t_v = vec![0u8; self.db.record_size];

        for partition_idx in 0..self.db.partition_count {
            let offset = offsets[partition_idx];
            let record = &self.db.data[partition_idx][offset];

            xor_in_place(&mut t_s, record);
            if masks[partition_idx] {
                xor_in_place(&mut t_v, record);
            }
        }

        PatchedQueryResponse {
            parities: ParityPair {
                rho_s: t_s,
                rho_v: t_v,
            },
        }
    }
}