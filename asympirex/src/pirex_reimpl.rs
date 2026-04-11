use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::{Aes128, Block};
use rand_chacha::ChaCha20Rng;
use rand_core::{Rng, SeedableRng};
use std::time::{Duration, Instant};

const KEY_SIZE: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct PirexParams {
    pub total_records: usize,
    pub record_size: usize,
    pub exponent: usize,
    pub group_log: usize,
    pub group_size: usize,
    pub group_count: usize,
    pub hint_count: usize,
    pub offset_bytes: usize,
    pub index_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct OfflineMetrics {
    pub client_compute_ms: f64,
    pub server_compute_ms: f64,
    pub client_outbound_bytes: usize,
    pub client_inbound_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct OnlineMetrics {
    pub wall_total_ms: f64,
    pub client_compute_total_ms: f64,
    pub client_compute_avg_ms: f64,
    pub server_compute_total_ms: f64,
    pub server_compute_avg_ms: f64,
    pub client_outbound_bytes: usize,
    pub client_inbound_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ReimplBenchmarkMetrics {
    pub params: PirexParams,
    pub query_count: usize,
    pub offline: OfflineMetrics,
    pub online: OnlineMetrics,
    pub client_storage_bytes: usize,
}

#[derive(Clone, Debug)]
struct HintEntry {
    key: [u8; KEY_SIZE],
    parity: Vec<u8>,
}

pub fn run_reimplemented_pirex_benchmark(
    total_records: usize,
    record_size: usize,
    query_count: usize,
) -> Result<ReimplBenchmarkMetrics, String> {
    let params = PirexParams::new(total_records, record_size)?;
    let records = build_records(params.total_records, params.record_size);
    let mut server = PirexServer::new(params, records.clone());
    let mut client = PirexClient::new(params);

    let offline_started = Instant::now();
    let offline = client.bootstrap(&mut server);
    let offline_wall_ms = offline_started.elapsed().as_secs_f64() * 1000.0;
    let offline = OfflineMetrics {
        client_compute_ms: offline.client_compute_ms.max(offline_wall_ms),
        server_compute_ms: offline.server_compute_ms,
        client_outbound_bytes: offline.client_outbound_bytes,
        client_inbound_bytes: offline.client_inbound_bytes,
    };

    let targets = build_query_targets(params.total_records, query_count);
    let online_started = Instant::now();
    let mut client_compute_total = 0.0f64;
    let mut server_compute_total = 0.0f64;
    let mut outbound_bytes = 0usize;
    let mut inbound_bytes = 0usize;

    for target in targets {
        let outcome = client.access(target, &mut server)?;
        let expected = &records[target];
        if outcome.recovered != *expected {
            return Err(format!("recovery mismatch for target {target}"));
        }

        client_compute_total += outcome.client_compute_ms;
        server_compute_total += outcome.server_compute_ms;
        outbound_bytes += outcome.client_outbound_bytes;
        inbound_bytes += outcome.client_inbound_bytes;
    }

    let wall_total_ms = online_started.elapsed().as_secs_f64() * 1000.0;
    let online = OnlineMetrics {
        wall_total_ms,
        client_compute_total_ms: client_compute_total,
        client_compute_avg_ms: client_compute_total / query_count as f64,
        server_compute_total_ms: server_compute_total,
        server_compute_avg_ms: server_compute_total / query_count as f64,
        client_outbound_bytes: outbound_bytes,
        client_inbound_bytes: inbound_bytes,
    };

    Ok(ReimplBenchmarkMetrics {
        params,
        query_count,
        offline,
        online,
        client_storage_bytes: client.storage_bytes(),
    })
}

impl PirexParams {
    pub fn new(total_records: usize, record_size: usize) -> Result<Self, String> {
        if total_records == 0 || (total_records & (total_records - 1)) != 0 {
            return Err("Pirex requires a power-of-two database size".to_string());
        }
        if record_size == 0 || record_size % 64 != 0 {
            return Err("Pirex requires record sizes that are multiples of 64 bytes".to_string());
        }

        let exponent = total_records.trailing_zeros() as usize;
        let group_log = (exponent / 2).max(1);
        let group_size = 1usize << group_log;
        let group_count = total_records / group_size;
        let hint_count = group_count * exponent.max(1);
        let offset_bytes = if group_log > 8 { 2 } else { 1 };
        let index_bytes = offset_bytes * 2;

        Ok(Self {
            total_records,
            record_size,
            exponent,
            group_log,
            group_size,
            group_count,
            hint_count,
            offset_bytes,
            index_bytes,
        })
    }

    fn partition(&self, index: usize) -> (usize, usize) {
        (index / self.group_size, index % self.group_size)
    }

    fn offset_mask(&self) -> usize {
        self.group_size - 1
    }
}

#[derive(Clone, Debug)]
struct BootstrapOutcome {
    client_compute_ms: f64,
    server_compute_ms: f64,
    client_outbound_bytes: usize,
    client_inbound_bytes: usize,
}

#[derive(Clone, Debug)]
struct AccessOutcome {
    recovered: Vec<u8>,
    client_compute_ms: f64,
    server_compute_ms: f64,
    client_outbound_bytes: usize,
    client_inbound_bytes: usize,
}

struct PirexClient {
    params: PirexParams,
    crypto: PirexCrypto,
    rng: ChaCha20Rng,
    hints: Vec<HintEntry>,
}

impl PirexClient {
    fn new(params: PirexParams) -> Self {
        Self {
            params,
            crypto: PirexCrypto::new(params),
            rng: ChaCha20Rng::from_seed([0x5Au8; 32]),
            hints: Vec::new(),
        }
    }

    fn bootstrap(&mut self, server: &mut PirexServer) -> BootstrapOutcome {
        let mut client_compute = Duration::ZERO;
        let mut server_compute = Duration::ZERO;
        let started = Instant::now();

        self.hints.clear();
        for _ in 0..self.params.hint_count {
            let mut key = [0u8; KEY_SIZE];
            self.rng.fill_bytes(&mut key);
            let (parity, prep_time) = server.prep(&key);
            self.hints.push(HintEntry { key, parity });
            server_compute += prep_time;
        }

        client_compute += started.elapsed();
        BootstrapOutcome {
            client_compute_ms: client_compute.as_secs_f64() * 1000.0,
            server_compute_ms: server_compute.as_secs_f64() * 1000.0,
            client_outbound_bytes: self.params.hint_count * KEY_SIZE,
            client_inbound_bytes: self.params.hint_count * self.params.record_size,
        }
    }

    fn access(&mut self, target: usize, server: &mut PirexServer) -> Result<AccessOutcome, String> {
        let (pk, offset) = self.params.partition(target);
        let started = Instant::now();
        let (key, hint_parity, search_ms) = self.search(pk, offset)?;
        let patch = self.patch(pk, &key);
        let (_, _, zeta_search_ms) = self.search(pk, patch.zeta_offset)?;

        let (resp0, t0) = server.parity(&patch.q0_offsets, false);
        let (resp1, t1) = server.parity(&patch.q0_indices, true);
        let (_resp2, t2) = server.parity(&patch.q1_offsets, false);
        let (resp3, t3) = server.parity(&patch.q1_indices, true);

        let recover_started = Instant::now();
        let recovered = xor_blocks(&[
            resp0.as_slice(),
            resp1.as_slice(),
            hint_parity.as_slice(),
            resp3.as_slice(),
        ]);
        let recover_ms = recover_started.elapsed().as_secs_f64() * 1000.0;

        let request_payload_len =
            patch.q0_offsets.len() + patch.q0_indices.len() + patch.q1_offsets.len() + patch.q1_indices.len();
        let request_framing_bytes = 4 + 4 * 4;
        let client_outbound_bytes = request_framing_bytes + request_payload_len + 1;
        let client_inbound_bytes = 1 + 4 * self.params.record_size;

        let client_compute_ms = started.elapsed().as_secs_f64() * 1000.0
            + search_ms
            + zeta_search_ms
            + patch.compute_ms
            + recover_ms;
        let server_compute_ms = ((t0 + t1 + t2 + t3).as_secs_f64() * 1000.0) / 2.0;

        Ok(AccessOutcome {
            recovered,
            client_compute_ms,
            server_compute_ms,
            client_outbound_bytes,
            client_inbound_bytes,
        })
    }

    fn search(&self, pk: usize, target_offset: usize) -> Result<([u8; KEY_SIZE], Vec<u8>, f64), String> {
        let started = Instant::now();
        for hint in &self.hints {
            if self.crypto.key_val(&hint.key, pk) == target_offset {
                return Ok((
                    hint.key,
                    hint.parity.clone(),
                    started.elapsed().as_secs_f64() * 1000.0,
                ));
            }
        }
        Err(format!("no hint for partition {pk} offset {target_offset}"))
    }

    fn patch(&mut self, pk: usize, key: &[u8; KEY_SIZE]) -> PatchBundle {
        let mut q1_offsets = vec![0u8; self.params.group_count * self.params.offset_bytes];
        self.rng.fill_bytes(&mut q1_offsets);

        let started = Instant::now();
        let (q0_indices, q1_indices, zeta_offset) = self.crypto.gen_ppr(pk, &mut self.rng);
        let mut q0_offsets = self.crypto.key_set(key);
        let zeta_bytes = encode_value(zeta_offset, self.params.offset_bytes);
        let pos = pk * self.params.offset_bytes;
        q0_offsets[pos..pos + self.params.offset_bytes].copy_from_slice(&zeta_bytes);

        PatchBundle {
            zeta_offset,
            q0_offsets,
            q0_indices,
            q1_offsets,
            q1_indices,
            compute_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }

    fn storage_bytes(&self) -> usize {
        self.hints
            .iter()
            .map(|hint| hint.key.len() + hint.parity.len())
            .sum()
    }
}

struct PatchBundle {
    zeta_offset: usize,
    q0_offsets: Vec<u8>,
    q0_indices: Vec<u8>,
    q1_offsets: Vec<u8>,
    q1_indices: Vec<u8>,
    compute_ms: f64,
}

struct PirexServer {
    params: PirexParams,
    records: Vec<Vec<u8>>,
}

impl PirexServer {
    fn new(params: PirexParams, records: Vec<Vec<u8>>) -> Self {
        Self { params, records }
    }

    fn prep(&mut self, key: &[u8; KEY_SIZE]) -> (Vec<u8>, Duration) {
        let started = Instant::now();
        let mut parity = vec![0u8; self.params.record_size];
        for pk in 0..self.params.group_count {
            let offset = PirexCrypto::new(self.params).key_val(key, pk);
            let index = pk * self.params.group_size + offset;
            xor_into(&mut parity, &self.records[index]);
        }
        (parity, started.elapsed())
    }

    fn parity(&mut self, arr: &[u8], full_indices: bool) -> (Vec<u8>, Duration) {
        let started = Instant::now();
        let mut parity = vec![0u8; self.params.record_size];
        let step = if full_indices {
            self.params.index_bytes
        } else {
            self.params.offset_bytes
        };

        for (pk, chunk) in arr.chunks(step).enumerate() {
            let index = if full_indices {
                let group = decode_value(&chunk[..self.params.offset_bytes]) % self.params.group_count;
                let offset = decode_value(&chunk[self.params.offset_bytes..]) & self.params.offset_mask();
                group * self.params.group_size + offset
            } else {
                let offset = decode_value(chunk) & self.params.offset_mask();
                pk * self.params.group_size + offset
            };
            xor_into(&mut parity, &self.records[index]);
        }

        (parity, started.elapsed())
    }
}

#[derive(Clone)]
struct PirexCrypto {
    params: PirexParams,
}

impl PirexCrypto {
    fn new(params: PirexParams) -> Self {
        Self { params }
    }

    fn key_val(&self, key: &[u8; KEY_SIZE], pk: usize) -> usize {
        let mut block = Block::default();
        let prefix = encode_value(pk, self.params.offset_bytes);
        block[..self.params.offset_bytes].copy_from_slice(&prefix);
        let cipher = Aes128::new(&GenericArray::clone_from_slice(key));
        cipher.encrypt_block(&mut block);
        decode_value(&block[..self.params.offset_bytes]) & self.params.offset_mask()
    }

    fn key_set(&self, key: &[u8; KEY_SIZE]) -> Vec<u8> {
        let mut offsets = Vec::with_capacity(self.params.group_count * self.params.offset_bytes);
        for pk in 0..self.params.group_count {
            offsets.extend_from_slice(&encode_value(
                self.key_val(key, pk),
                self.params.offset_bytes,
            ));
        }
        offsets
    }

    fn gen_ppr(&self, pk: usize, rng: &mut ChaCha20Rng) -> (Vec<u8>, Vec<u8>, usize) {
        let mut random_chunks = vec![0u8; self.params.group_count * self.params.index_bytes];
        rng.fill_bytes(&mut random_chunks);

        let zeta_start = pk * self.params.index_bytes + self.params.offset_bytes;
        let zeta_end = zeta_start + self.params.offset_bytes;
        let zeta_offset = decode_value(&random_chunks[zeta_start..zeta_end]) & self.params.offset_mask();

        let mut q0_indices = Vec::new();
        let mut q1_indices = Vec::new();
        for group in 0..self.params.group_count {
            let start = group * self.params.index_bytes;
            let end = start + self.params.index_bytes;
            let chunk = &mut random_chunks[start..end];
            let rbit = chunk[0];
            let group_prefix = encode_value(group, self.params.offset_bytes);
            chunk[..self.params.offset_bytes].copy_from_slice(&group_prefix);
            if group == pk {
                q1_indices.extend_from_slice(chunk);
            } else if rbit & 1 == 1 {
                q0_indices.extend_from_slice(chunk);
                q1_indices.extend_from_slice(chunk);
            }
        }

        (q0_indices, q1_indices, zeta_offset)
    }
}

fn build_records(total_records: usize, record_size: usize) -> Vec<Vec<u8>> {
    (0..total_records)
        .map(|index| {
            let base = (index as u8).wrapping_mul(17).wrapping_add(3);
            vec![base; record_size]
        })
        .collect()
}

fn build_query_targets(total_records: usize, query_count: usize) -> Vec<usize> {
    (0..query_count)
        .map(|idx| (idx * 7 + 5) % total_records)
        .collect()
}

fn encode_value(value: usize, byte_len: usize) -> Vec<u8> {
    let be = (value as u64).to_be_bytes();
    be[be.len() - byte_len..].to_vec()
}

fn decode_value(bytes: &[u8]) -> usize {
    let mut padded = [0u8; 8];
    let start = padded.len() - bytes.len();
    padded[start..].copy_from_slice(bytes);
    u64::from_be_bytes(padded) as usize
}

fn xor_into(target: &mut [u8], source: &[u8]) {
    for (dst, src) in target.iter_mut().zip(source.iter()) {
        *dst ^= *src;
    }
}

fn xor_blocks(blocks: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![0u8; blocks[0].len()];
    for block in blocks {
        xor_into(&mut out, block);
    }
    out
}
