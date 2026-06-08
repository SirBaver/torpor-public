// supervisor_arith.rs — S1 : superviseur déterministe (Rust pur, sans LLM).
//
// Reçoit un message [n: u8, claim: u8] où :
//   n     = le nombre à tester
//   claim = 1 si le worker a affirmé que n est premier, 0 sinon
//
// Calcule is_prime(n) de façon déterministe, compare avec claim,
// émet [verdict: u8] (0=Approved, 1=Rejected) dans le log causal,
// puis termine.
//
// La nature déterministe du superviseur garantit la reproductibilité
// du verdict indépendamment de toute variabilité LLM (D-F, doc/poc_E2E.md §1.3).
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example supervisor_arith
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, terminate};

/// Teste si n est premier (algorithme trial division, borne O(√n)).
fn is_prime(n: u32) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 {
        return true;
    }
    if n % 2 == 0 {
        return false;
    }
    let mut i = 3u32;
    while i.saturating_mul(i) <= n {
        if n % i == 0 {
            return false;
        }
        i += 2;
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len < 2 {
        terminate();
        return;
    }
    // Lecture via pointeur brut (from_raw_parts panique sur ptr=0 en debug).
    let p = ptr as *const u8;
    let n = p.read() as u32;
    let claim = p.add(1).read() != 0;

    let actual = is_prime(n);
    // 0 = Approved (claim correct), 1 = Rejected (claim incorrect)
    let verdict: u8 = if claim == actual { 0 } else { 1 };

    barrier();
    emit_raw(1, &[verdict]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
