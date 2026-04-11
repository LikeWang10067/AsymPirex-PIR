// src/client.rs

use std::collections::{HashMap, VecDeque};

use rand_chacha::ChaCha20Rng;
use rand_core::{Rng, SeedableRng};
use serde::Serialize;

use crate::crypto::{derive_hint_seed, derive_labeled_seed, expand_hint, HintExpansion};
use crate::server::{
    BootstrapRequest, BootstrapResponse, HintId, HintServer, ParityPair, PatchedQueryRequest,
    QueryServer, RefreshRequest, ServerHintPacket,
};
use crate::utils::{xor_blocks, xor_in_place};

pub type DatabaseIndex = usize;

#[derive(Default)]
pub struct ClientTable {
    // mapping: database index -> list of active hint IDs covering it.
    // our Hint-membership table
    pub table: HashMap<DatabaseIndex, Vec<HintId>>,
}

#[derive(Clone, Debug)]
pub struct ClientHint {
    pub parities: ParityPair,
    /// Partition-level overrides from swap operations: (partition_idx, new_local_offset).
    pub patches: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct QueryOutcome {
    pub consumed_hint_id: HintId,
    pub swap_hint_id: Option<HintId>,
    pub target_index: DatabaseIndex,
    pub replacement_index: DatabaseIndex,
    pub target_record: Vec<u8>,
    pub replacement_record: Vec<u8>,
    pub sent_query: PatchedQueryRequest,
    pub refresh_triggered: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct MaintenanceOutcome {
    pub promoted_hint: bool,
    pub refresh_triggered: bool,
    pub fetched_hint_count: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct HintSnapshot {
    pub hint_id: HintId,
    pub indices: Vec<DatabaseIndex>,
    pub offsets: Vec<usize>,
    pub masks: Vec<bool>,
    pub rho_s: Vec<u8>,
    pub rho_v: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BufferedHintSnapshot {
    pub hint_id: HintId,
    pub rho_s: Vec<u8>,
    pub rho_v: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MembershipEntry {
    pub db_index: DatabaseIndex,
    pub hint_ids: Vec<HintId>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ClientStateSnapshot {
    pub active_hints: Vec<HintSnapshot>,
    pub hint_buffer: Vec<BufferedHintSnapshot>,
    pub membership_table: Vec<MembershipEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SnapshotDelta<T> {
    pub removed: Vec<T>,
    pub added: Vec<T>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClientStateDelta {
    pub active_hints: SnapshotDelta<HintSnapshot>,
    pub hint_buffer: SnapshotDelta<BufferedHintSnapshot>,
    pub membership_table: SnapshotDelta<MembershipEntry>,
}

pub struct Client {
    client_id: String,
    usk: [u8; 32],
    n: usize,
    m: usize,
    active_hint_count: usize,
    refresh_batch_size: usize,
    next_hint_id: HintId,
    queries_since_refresh: usize,
    rng: ChaCha20Rng,
    pub table: ClientTable,
    pub active_hints: HashMap<HintId, ClientHint>,
    pub hbuffer: VecDeque<ServerHintPacket>,
}

impl ClientTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_hint(&mut self, hint_id: HintId, indices: &[DatabaseIndex]) {
        for &idx in indices {
            self.table.entry(idx).or_default().push(hint_id);
        }
    }

    pub fn remove_hint(&mut self, hint_id: HintId, indices: &[DatabaseIndex]) {
        for &idx in indices {
            if let Some(hints) = self.table.get_mut(&idx) {
                hints.retain(|&id| id != hint_id);
                if hints.is_empty() {
                    self.table.remove(&idx);
                }
            }
        }
    }

    pub fn replace_index(&mut self, hint_id: HintId, old_idx: DatabaseIndex, new_idx: DatabaseIndex) {
        if let Some(hints) = self.table.get_mut(&old_idx) {
            hints.retain(|&id| id != hint_id);
            if hints.is_empty() {
                self.table.remove(&old_idx);
            }
        }

        self.table.entry(new_idx).or_default().push(hint_id);
    }

    pub fn random_covering_hint(
        &self,
        target: DatabaseIndex,
        rng: &mut ChaCha20Rng,
    ) -> Option<HintId> {
        let hints = self.table.get(&target)?;
        if hints.is_empty() {
            None
        } else {
            Some(hints[(rng.next_u64() as usize) % hints.len()])
        }
    }

    pub fn random_alternate_hint(
        &self,
        target: DatabaseIndex,
        excluded: HintId,
        rng: &mut ChaCha20Rng,
    ) -> Option<HintId> {
        let hints = self.table.get(&target)?;
        let candidates: Vec<_> = hints
            .iter()
            .copied()
            .filter(|&hint_id| hint_id != excluded)
            .collect();

        if candidates.is_empty() {
            None
        } else {
            Some(candidates[(rng.next_u64() as usize) % candidates.len()])
        }
    }
}

fn reconstruct_hint_state(
    usk: &[u8; 32],
    hint_id: HintId,
    patches: &[(usize, usize)],
    n: usize,
    m: usize,
) -> HintExpansion {
    let seed = derive_hint_seed(usk, hint_id);
    let mut exp = expand_hint(seed, n, m);
    for &(partition, new_offset) in patches {
        exp.offsets[partition] = new_offset;
        exp.indices[partition] = partition * m + new_offset;
    }
    exp
}

impl Client {
    pub fn from_bootstrap(
        client_id: impl Into<String>,
        n: usize,
        m: usize,
        active_hint_count: usize,
        refresh_batch_size: usize,
        bootstrap: BootstrapResponse,
    ) -> Self {
        let client_id = client_id.into();
        let rng_seed = derive_labeled_seed(&bootstrap.usk, b"client-local-rng");
        let mut client = Self {
            client_id,
            usk: bootstrap.usk,
            n,
            m,
            active_hint_count,
            refresh_batch_size,
            next_hint_id: bootstrap.hint_packets.len(),
            queries_since_refresh: 0,
            rng: ChaCha20Rng::from_seed(rng_seed),
            table: ClientTable::new(),
            active_hints: HashMap::new(),
            hbuffer: VecDeque::new(),
        };

        for packet in bootstrap.hint_packets {
            if client.active_hints.len() < active_hint_count {
                client.activate_hint_packet(packet);
            } else {
                client.hbuffer.push_back(packet);
            }
        }

        client
    }

    pub fn active_hint_len(&self) -> usize {
        self.active_hints.len()
    }

    pub fn buffer_len(&self) -> usize {
        self.hbuffer.len()
    }

    pub fn query(
        &mut self,
        target_index: DatabaseIndex,
        query_server: &QueryServer<'_>,
        hint_server: &HintServer<'_>,
    ) -> Result<QueryOutcome, String> {
        let mut outcome = self.query_online_only(target_index, query_server)?;
        let maintenance = self.run_maintenance(hint_server)?;
        outcome.refresh_triggered = maintenance.refresh_triggered;
        Ok(outcome)
    }

    pub fn query_online_only(
        &mut self,
        target_index: DatabaseIndex,
        query_server: &QueryServer<'_>,
    ) -> Result<QueryOutcome, String> {
        if target_index >= self.n * self.m {
            return Err(format!("target index {target_index} is out of range"));
        }

        let consumed_hint_id = self
            .table
            .random_covering_hint(target_index, &mut self.rng)
            .ok_or_else(|| format!("no active hint currently covers target index {target_index}"))?;

        let consumed_hint = self
            .active_hints
            .get(&consumed_hint_id)
            .cloned()
            .ok_or_else(|| format!("missing active hint state for hint {consumed_hint_id}"))?;

        let exp = reconstruct_hint_state(
            &self.usk, consumed_hint_id, &consumed_hint.patches, self.n, self.m,
        );

        let target_partition = target_index / self.m;
        let replacement_offset = (self.rng.next_u64() as usize) % self.m;
        let replacement_index = target_partition * self.m + replacement_offset;

        let mut patched_offsets = exp.offsets;
        let mut patched_masks = exp.masks.clone();
        patched_offsets[target_partition] = replacement_offset;
        patched_masks[target_partition] = !patched_masks[target_partition];

        let sent_query = PatchedQueryRequest {
            offsets: patched_offsets,
            masks: patched_masks,
        };
        let server_response = query_server.handle_query(sent_query.clone());
        let delta_s = xor_blocks(&consumed_hint.parities.rho_s, &server_response.parities.rho_s);
        let delta_v = xor_blocks(&consumed_hint.parities.rho_v, &server_response.parities.rho_v);

        let original_mask_bit = exp.masks[target_partition];
        let (target_record, replacement_record) = if original_mask_bit {
            let target_record = delta_v.clone();
            let replacement_record = xor_blocks(&delta_s, &delta_v);
            (target_record, replacement_record)
        } else {
            let replacement_record = delta_v.clone();
            let target_record = xor_blocks(&delta_s, &delta_v);
            (target_record, replacement_record)
        };

        let mut swap_hint_id = None;
        if replacement_index != target_index {
            let selected_swap_hint_id = self
                .table
                .random_alternate_hint(replacement_index, consumed_hint_id, &mut self.rng)
                .ok_or_else(|| {
                    format!(
                        "no alternate active hint available to replace index {replacement_index}"
                    )
                })?;

            self.swap_hint_coverage(
                selected_swap_hint_id,
                replacement_index,
                target_index,
                &replacement_record,
                &target_record,
            )?;
            swap_hint_id = Some(selected_swap_hint_id);
        }

        self.consume_hint(consumed_hint_id)?;
        self.promote_next_buffered_hint()?;
        self.queries_since_refresh += 1;

        Ok(QueryOutcome {
            consumed_hint_id,
            swap_hint_id,
            target_index,
            replacement_index,
            target_record,
            replacement_record,
            sent_query,
            refresh_triggered: false,
        })
    }

    pub fn run_maintenance(
        &mut self,
        hint_server: &HintServer<'_>,
    ) -> Result<MaintenanceOutcome, String> {
        let mut outcome = MaintenanceOutcome::default();

        if self.active_hints.len() < self.active_hint_count && !self.hbuffer.is_empty() {
            self.promote_next_buffered_hint()?;
            outcome.promoted_hint = true;
        }

        if self.queries_since_refresh < self.refresh_batch_size && !self.hbuffer.is_empty() {
            return Ok(outcome);
        }

        let batch_size = self.refresh_batch_size.max(1);
        let request = RefreshRequest {
            client_id: self.client_id.clone(),
            next_hint_id: self.next_hint_id,
            batch_size,
        };
        let response = hint_server.handle_refresh(request);
        let fetched_hint_count = response.hint_packets.len();
        self.next_hint_id += fetched_hint_count;
        self.hbuffer.extend(response.hint_packets);
        self.queries_since_refresh = 0;

        outcome.refresh_triggered = fetched_hint_count > 0;
        outcome.fetched_hint_count = fetched_hint_count;

        if self.active_hints.len() < self.active_hint_count && !self.hbuffer.is_empty() {
            self.promote_next_buffered_hint()?;
            outcome.promoted_hint = true;
        }

        Ok(outcome)
    }

    pub fn bootstrap_from_hint_server(
        client_id: impl Into<String>,
        n: usize,
        m: usize,
        active_hint_count: usize,
        refresh_batch_size: usize,
        hint_server: &HintServer<'_>,
    ) -> Self {
        let client_id = client_id.into();
        let request = BootstrapRequest {
            client_id: client_id.clone(),
            total_hints: active_hint_count + refresh_batch_size,
        };
        let response = hint_server.handle_bootstrap(request);
        Self::from_bootstrap(
            client_id,
            n,
            m,
            active_hint_count,
            refresh_batch_size,
            response,
        )
    }

    pub fn snapshot(&self) -> ClientStateSnapshot {
        let mut active_ids: Vec<_> = self.active_hints.keys().copied().collect();
        active_ids.sort_unstable();
        let active_hints = active_ids
            .into_iter()
            .map(|hint_id| {
                let hint = self
                    .active_hints
                    .get(&hint_id)
                    .expect("active hint id must resolve");
                let exp = reconstruct_hint_state(
                    &self.usk, hint_id, &hint.patches, self.n, self.m,
                );
                HintSnapshot {
                    hint_id,
                    indices: exp.indices,
                    offsets: exp.offsets,
                    masks: exp.masks,
                    rho_s: hint.parities.rho_s.clone(),
                    rho_v: hint.parities.rho_v.clone(),
                }
            })
            .collect();

        let hint_buffer = self
            .hbuffer
            .iter()
            .map(|packet| BufferedHintSnapshot {
                hint_id: packet.hint_id,
                rho_s: packet.parities.rho_s.clone(),
                rho_v: packet.parities.rho_v.clone(),
            })
            .collect();

        let mut membership_entries: Vec<_> = self.table.table.iter().collect();
        membership_entries.sort_by_key(|(index, _)| **index);
        let membership_table = membership_entries
            .into_iter()
            .map(|(index, hint_ids)| {
                let mut hint_ids = hint_ids.clone();
                hint_ids.sort_unstable();
                MembershipEntry {
                    db_index: *index,
                    hint_ids,
                }
            })
            .collect();

        ClientStateSnapshot {
            active_hints,
            hint_buffer,
            membership_table,
        }
    }

    pub fn snapshot_delta(
        before: &ClientStateSnapshot,
        after: &ClientStateSnapshot,
    ) -> ClientStateDelta {
        ClientStateDelta {
            active_hints: snapshot_delta_vec(&before.active_hints, &after.active_hints),
            hint_buffer: snapshot_delta_vec(&before.hint_buffer, &after.hint_buffer),
            membership_table: snapshot_delta_vec(&before.membership_table, &after.membership_table),
        }
    }

    fn activate_hint_packet(&mut self, packet: ServerHintPacket) {
        let seed = derive_hint_seed(&self.usk, packet.hint_id);
        let expansion = expand_hint(seed, self.n, self.m);

        let hint = ClientHint {
            parities: packet.parities,
            patches: Vec::new(),
        };

        self.table.register_hint(packet.hint_id, &expansion.indices);
        self.active_hints.insert(packet.hint_id, hint);
    }

    fn promote_next_buffered_hint(&mut self) -> Result<(), String> {
        if self.active_hints.len() >= self.active_hint_count {
            return Ok(());
        }

        let packet = self
            .hbuffer
            .pop_front()
            .ok_or_else(|| "hint buffer is empty and cannot replenish the active set".to_string())?;

        self.activate_hint_packet(packet);
        Ok(())
    }

    fn consume_hint(&mut self, hint_id: HintId) -> Result<(), String> {
        let removed = self
            .active_hints
            .remove(&hint_id)
            .ok_or_else(|| format!("attempted to consume unknown hint {hint_id}"))?;

        let exp = reconstruct_hint_state(&self.usk, hint_id, &removed.patches, self.n, self.m);
        self.table.remove_hint(hint_id, &exp.indices);
        Ok(())
    }

    fn swap_hint_coverage(
        &mut self,
        hint_id: HintId,
        old_index: DatabaseIndex,
        new_index: DatabaseIndex,
        old_record: &[u8],
        new_record: &[u8],
    ) -> Result<(), String> {
        let partition = old_index / self.m;

        let mask_bit = {
            let hint = self
                .active_hints
                .get(&hint_id)
                .ok_or_else(|| format!("missing replacement hint state for hint {hint_id}"))?;
            let exp = reconstruct_hint_state(&self.usk, hint_id, &hint.patches, self.n, self.m);
            if exp.indices[partition] != old_index {
                return Err(format!(
                    "hint {hint_id} does not cover index {old_index} in partition {partition}"
                ));
            }
            exp.masks[partition]
        };

        let hint = self.active_hints.get_mut(&hint_id).unwrap();

        xor_in_place(&mut hint.parities.rho_s, old_record);
        xor_in_place(&mut hint.parities.rho_s, new_record);

        if mask_bit {
            xor_in_place(&mut hint.parities.rho_v, old_record);
            xor_in_place(&mut hint.parities.rho_v, new_record);
        }

        let new_offset = new_index % self.m;
        if let Some(existing) = hint.patches.iter_mut().find(|(p, _)| *p == partition) {
            existing.1 = new_offset;
        } else {
            hint.patches.push((partition, new_offset));
        }

        self.table.replace_index(hint_id, old_index, new_index);

        Ok(())
    }

}

fn snapshot_delta_vec<T>(before: &[T], after: &[T]) -> SnapshotDelta<T>
where
    T: Clone + PartialEq,
{
    let removed = before
        .iter()
        .filter(|entry| !after.contains(entry))
        .cloned()
        .collect();
    let added = after
        .iter()
        .filter(|entry| !before.contains(entry))
        .cloned()
        .collect();

    SnapshotDelta { removed, added }
}