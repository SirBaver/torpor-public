// worker_prime.rs — S1 : agent LLM qui teste si un nombre est premier.
//
// Protocole (deux phases séparées, une par Message::Data) :
//   Phase 0x01 [n: u8] — appelle agent_infer, émet la revendication provisoire,
//                         demande validation via A3 (request_validation(1)).
//   Phase 0x02         — lit le verdict (get_verdict) et émet le résultat final.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example worker_prime
//
// L'agent ne gère pas les erreurs de manière exhaustive — c'est un PoC.
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{
    barrier, emit_raw, infer, request_validation, get_verdict, terminate,
    INFER_RESPONSE_BUF_LEN,
};

// State persisté entre les deux appels à process().
static mut CLAIM: u8 = 0; // 1 = claimed prime, 0 = claimed not prime

/// Cherche `{"is_prime": true}` dans le texte de réponse LLM.
/// Heuristique : cherche "is_prime" puis compare la position de "true" vs "false".
unsafe fn parse_is_prime(resp: &[u8]) -> bool {
    // Cherche "is_prime" dans le buffer
    if let Some(pos) = resp.windows(8).position(|w| w == b"is_prime") {
        let after = &resp[pos..];
        let t = after.windows(4).position(|w| w == b"true").unwrap_or(usize::MAX);
        let f = after.windows(5).position(|w| w == b"false").unwrap_or(usize::MAX);
        return t < f;
    }
    // Fallback : tout "true" dans la réponse
    resp.windows(4).any(|w| w == b"true")
}

/// Écrit `n` (u8) en ASCII décimal dans `buf` à partir de `pos`. Retourne la nouvelle position.
fn write_decimal(buf: &mut [u8], pos: usize, n: u8) -> usize {
    let mut p = pos;
    if n >= 100 {
        buf[p] = b'0' + n / 100;
        p += 1;
    }
    if n >= 10 {
        buf[p] = b'0' + (n / 10 % 10);
        p += 1;
    }
    buf[p] = b'0' + (n % 10);
    p + 1
}

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 {
        return;
    }
    // Lecture via pointeur brut : from_raw_parts panique en debug sur ptr=0
    // (null-pointer check), mais l'adresse 0 est valide dans la mémoire linéaire WASM.
    let p = ptr as *const u8;
    let cmd = p.read();
    match cmd {
        0x01 => {
            let n = if len > 1 { p.add(1).read() } else { 39 };
            phase_infer_and_validate(n);
        }
        0x02 => phase_read_verdict(),
        _ => terminate(),
    }
}

unsafe fn phase_infer_and_validate(n: u8) {
    // Construire le prompt sans allocation heap
    let mut prompt_buf = [0u8; 256];
    let prefix = b"Is ";
    let suffix = b" a prime number? Answer with JSON only: {\"is_prime\": true} or {\"is_prime\": false}";
    let mut pos = 0;
    for &b in prefix.iter() {
        prompt_buf[pos] = b;
        pos += 1;
    }
    pos = write_decimal(&mut prompt_buf, pos, n);
    for &b in suffix.iter() {
        prompt_buf[pos] = b;
        pos += 1;
    }
    let prompt = &prompt_buf[..pos];

    // Appel LLM (bloque jusqu'à réponse, timeout, ou cancellation)
    let mut resp_buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let resp_len = match infer(prompt, &mut resp_buf, 60_000) {
        Ok(n) => n,
        Err(_) => {
            // Inférence échouée (Timeout, Cancelled, Error) → abandon
            terminate();
            return;
        }
    };
    let resp = &resp_buf[..resp_len];

    // Heuristique : extraire le booléen is_prime de la réponse JSON
    let claimed_prime = parse_is_prime(resp);
    CLAIM = if claimed_prime { 1 } else { 0 };

    // Émettre la revendication provisoire dans le log causal
    barrier();
    if claimed_prime {
        emit_raw(1, b"provisional:{\"is_prime\":true}");
    } else {
        emit_raw(1, b"provisional:{\"is_prime\":false}");
    }

    // A3 — demander validation avec risque moyen (1)
    // L'agent passe en AwaitingValidation ; process() retourne.
    // Le run_loop attend Message::ValidationResponse avant de continuer.
    request_validation(1);
}

unsafe fn phase_read_verdict() {
    // 0=Approved, 1=Rejected, 2=Timeout
    let verdict = get_verdict();
    barrier();
    match verdict {
        1 => emit_raw(1, b"{\"is_prime\":null,\"reason\":\"validation_rejected\"}"),
        2 => emit_raw(1, b"{\"is_prime\":null,\"reason\":\"validation_timeout\"}"),
        _ => {
            // Approved — confirmer la revendication originale
            if CLAIM == 1 {
                emit_raw(1, b"{\"is_prime\":true,\"reason\":\"approved\"}");
            } else {
                emit_raw(1, b"{\"is_prime\":false,\"reason\":\"approved\"}");
            }
        }
    }
    terminate();
}

#[allow(dead_code)]
fn main() {}
