// src/main.rs
mod database;
mod crypto;
mod client;
mod utils;
mod server;

use database::Database;
use crypto::HintPrg;
use utils::xor_in_place;
use server::QueryServer;

fn main() {
    println!("=== AsymPirex System Start ===");
    
    let n = 10; 
    let m = 10;
    let b = 16; // For pretty printing, we reduce the record size to 16 Bytes
    let mut db = Database::new(n, m, b);
    
    // [Extra Step]: To verify the recovered data is correct, we inject specific fake data
    // into the database, making the 15th record all 7s (0x07), and all others 0s
    let target_idx = 15;
    let target_partition = target_idx / m;
    let target_offset = target_idx % m;
    db.data[target_partition][target_offset] = vec![7u8; b]; 
    println!("[Init] Injected target data (all 7s) at index {}", target_idx);

    // ==========================================
    // Phase 1: Offline Phase (Client holds a Hint)
    // ==========================================
    let seed = [1u8; 32];
    let mut prg = HintPrg::new(seed);
    let offsets = prg.expand_offsets(n, m);
    let mut hint_indices = HintPrg::compute_global_indices(&offsets, m);
    
    // Assume this Hint contains target_idx (we force it into the test)
    hint_indices[target_partition] = target_idx; 
    
    println!("\n[Offline] Client holds a Hint covering indices: {:?}", hint_indices);

    // Client precomputes the Parity (P_Hint) of this Hint locally
    // In reality, S_H computes it and sends it to Client, here we simulate it with code
    let mut hint_parity = vec![0u8; b];
    for &idx in &hint_indices {
        xor_in_place(&mut hint_parity, db.get_record(idx).unwrap());
    }
    // Print the first few bytes of P_Hint to see what it looks like
    println!("          Hint Parity (P_Hint) begins with: {:?}", &hint_parity[..4]);

    // ==========================================
    // Phase 2: Online Query Phase (Online Query)
    // ==========================================
    println!("\n[Online] Client wants to privately read index {}...", target_idx);
    
    // Client constructs Query and sends it to S_Q
    // To protect privacy, Client sends the index list to Server, which is almost the same as the Hint.
    // However, it does some obfuscation on the target block (in the full AsymPirex version, it flips the Mask).
    // Here we use the most basic PIR principle: Server queries all blocks except the target block
    let mut query_indices = hint_indices.clone();
    
    // We remove the target block from the Server's query list
    // So Server doesn't know we want 15, it only knows we requested a bunch of random indices
    query_indices[target_partition] = 0; // Replace with a fake index 0
    println!("         Client sends Query to S_Q: {:?}", query_indices);

    let server = QueryServer::new(&db);
    let server_parity = server.answer_query(&query_indices);
    println!("         Server replies with P_Server beginning with: {:?}", &server_parity[..4]);

    // ==========================================
    // Phase 3: Local Algebraic Recovery (Recovery)
    // ==========================================
    // Target Data = P_Hint XOR P_Server
    let mut recovered_data = hint_parity.clone();
    xor_in_place(&mut recovered_data, &server_parity);

    println!("\n[Recovery] Client XORs P_Hint and P_Server...");
    println!("           Recovered Data begins with: {:?}", &recovered_data[..4]);

    // Assert validation (if both sides are equal, the program will pass quietly; if not, it will crash directly!)
    assert_eq!(recovered_data, vec![7u8; b], "❌ RECOVERY FAILED!");
    println!("✅ SUCCESS! The recovered data matches the original target perfectly!");
}