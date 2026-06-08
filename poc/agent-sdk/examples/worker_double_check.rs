// worker_double_check.rs — S2 : composition A1+A2 sur incohérence LLM.
//
// Protocole (une seule phase) :
//   process([n: u8]) :
//     1. agent_infer → revendication LLM sur is_prime(n)
//     2. barrier + emit ActionResult provisoire (seq++)
//     3. agent_introspect (A1) → observe seq courant
//     4. barrier + emit Introspect (seq++)
//     5. Vérification interne déterministe : is_prime_rust(n)
//     6a. Si LLM incorrect → agent_self_rollback(1) (A2) → barrier + emit résultat corrigé
//     6b. Si LLM correct  → barrier + emit résultat confirmé
//
// Avec FixedResponseBackend({"is_prime": true}) et n=39 (non premier) :
// le LLM se trompe → chemin 6a est emprunté de façon déterministe.
//
// Assert S2 : Introspect (0x06) puis SelfRollback (0x07) présents dans le log,
//             résultat final = "self_rollback_after_llm_error".
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example worker_double_check
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{
    barrier, emit_raw, infer, introspect, self_rollback, terminate,
    INFER_RESPONSE_BUF_LEN, INTROSPECT_LEN,
};

fn is_prime(n: u32) -> bool {
    if n < 2 { return false; }
    if n == 2 { return true; }
    if n % 2 == 0 { return false; }
    let mut i = 3u32;
    while i.saturating_mul(i) <= n {
        if n % i == 0 { return false; }
        i += 2;
    }
    true
}

unsafe fn parse_is_prime(resp: &[u8]) -> bool {
    if let Some(pos) = resp.windows(8).position(|w| w == b"is_prime") {
        let after = &resp[pos..];
        let t = after.windows(4).position(|w| w == b"true").unwrap_or(usize::MAX);
        let f = after.windows(5).position(|w| w == b"false").unwrap_or(usize::MAX);
        return t < f;
    }
    resp.windows(4).any(|w| w == b"true")
}

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 {
        terminate();
        return;
    }
    let p = ptr as *const u8;
    let n = p.read() as u32;

    // Étape 1 — appel LLM
    let mut resp_buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let resp_len = match infer(
        b"Is this number prime? Answer JSON only: {\"is_prime\": true} or {\"is_prime\": false}",
        &mut resp_buf,
        60_000,
    ) {
        Ok(n) => n,
        Err(_) => { terminate(); return; }
    };
    let llm_claim = parse_is_prime(&resp_buf[..resp_len]);

    // Étape 2 — émettre la revendication provisoire (barrier #1, seq → 1)
    barrier();
    if llm_claim {
        emit_raw(1, b"provisional:{\"is_prime\":true}");
    } else {
        emit_raw(1, b"provisional:{\"is_prime\":false}");
    }

    // Étape 3 — A1 : introspect (ne modifie pas seq)
    let mut intro_buf = [0u8; INTROSPECT_LEN];
    introspect(&mut intro_buf);

    // Étape 4 — émettre l'introspect dans le log (barrier #2, seq → 2 ; emit_type=6=Introspect)
    barrier();
    emit_raw(6, &intro_buf);

    // Étape 5 — vérification interne déterministe
    let ground_truth = is_prime(n);

    if llm_claim != ground_truth {
        // Étape 6a — A2 : LLM incorrect → rollback(1) annule le snapshot de la revendication
        // seq=2 ≥ 1+1 → depth=1 est valide
        self_rollback(1);
        // Après le rollback, émettre le résultat corrigé (barrier #3 depuis snapshot restauré)
        barrier();
        emit_raw(1, b"{\"is_prime\":null,\"reason\":\"self_rollback_after_llm_error\"}");
    } else {
        // Étape 6b — LLM correct : confirmer la revendication
        barrier();
        if llm_claim {
            emit_raw(1, b"{\"is_prime\":true,\"reason\":\"llm_correct\"}");
        } else {
            emit_raw(1, b"{\"is_prime\":false,\"reason\":\"llm_correct\"}");
        }
    }

    terminate();
}

#[allow(dead_code)]
fn main() {}
