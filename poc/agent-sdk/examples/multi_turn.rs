// multi_turn.rs — agent conversationnel multi-tour (ADR-0012).
//
// Maintient l'historique de la session en mémoire WASM (cache volatile).
// Chaque process() = un tour utilisateur. L'agent ne termine pas entre les tours.
//
// Décisions architect (2026-05-30) :
//   - commit_barrier par tour : P6 au grain du tour, P2 borné (ADR-0051/0019)
//   - évinçable entre tours : historique en mémoire = cache, pas état autoritaire (P1a)
//   - réponse LLM hors state_bytes : P5 reste trivial (ADR-0053 §D-P5)
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example multi_turn --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, INFER_RESPONSE_BUF_LEN};

// Historique de la session courante — cache volatile, perdu à l'éviction.
// Format linéaire : "Human: <msg>\nAssistant: <resp>\n" répété.
// Non persisté dans l'état hashé (ADR-0053 §D-P5 Branche NON).
static mut HISTORY: Vec<u8> = Vec::new();

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 {
        return;
    }
    let user_bytes = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    // Construire le prompt : historique + tour courant
    let mut prompt: Vec<u8> = Vec::with_capacity(HISTORY.len() + user_bytes.len() + 32);
    prompt.extend_from_slice(&HISTORY);
    prompt.extend_from_slice(b"Human: ");
    prompt.extend_from_slice(user_bytes);
    prompt.extend_from_slice(b"\nAssistant:");

    let mut resp_buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let resp_len = match infer(&prompt, &mut resp_buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            // Décision architect : commit_barrier par tour même en erreur
            barrier();
            emit_raw(1, b"[inference error]");
            return;
        }
    };

    let response = &resp_buf[..resp_len];

    // Mettre à jour le cache historique (hors état hashé)
    HISTORY.extend_from_slice(b"Human: ");
    HISTORY.extend_from_slice(user_bytes);
    HISTORY.extend_from_slice(b"\nAssistant:");
    HISTORY.extend_from_slice(response);
    HISTORY.extend_from_slice(b"\n");

    // Décision architect : un commit_barrier par tour
    barrier();
    // Émettre la réponse (EmitType::ActionResult = 1)
    emit_raw(1, response);
    // Pas de terminate() — l'agent reste vivant pour le prochain tour
}

#[allow(dead_code)]
fn main() {}
