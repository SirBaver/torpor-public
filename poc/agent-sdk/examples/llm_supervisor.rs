// llm_supervisor.rs — agent superviseur qui évalue la réponse du worker.
//
// Protocole :
//   Reçoit [payload: réponse provisoire à évaluer].
//   Appelle agent_infer pour décider : approuver (0x00) ou rejeter (0x01).
//   Émet le verdict comme ActionResult byte.
//   Termine.
//
// Décision verdict (architect 2026-05-31, L97) — variante A' :
//   Prompt demande évaluation libre + dernier mot = APPROVE ou REJECT.
//   Décodage : isoler le dernier token non-blanc, strip ponctuation finale,
//   tester == "REJECT" (insensible casse). Tout le reste → approve (fail-open).
//   Fail-open cohérent avec runner (timeout→0x00) et ADR-0006 (supervision humaine fréquente).
//   REJECT est le discriminant car c'est l'action à conséquence : on exige une
//   assertion positive de rejet ; l'ambiguïté retombe sur l'approbation (rattrapable).
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example llm_supervisor --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

/// Extrait le dernier token non-blanc du buffer, strip la ponctuation finale.
/// Retourne true si ce token est "REJECT" (insensible à la casse).
fn last_token_is_reject(buf: &[u8]) -> bool {
    // Trouver la fin (ignorer whitespace + ponctuation finale)
    let mut end = buf.len();
    while end > 0 {
        match buf[end - 1] {
            b' ' | b'\n' | b'\r' | b'\t' | b'.' | b'!' | b',' | b':' => end -= 1,
            _ => break,
        }
    }
    // Trouver le début du dernier token (reculer jusqu'au whitespace)
    let mut start = end;
    while start > 0 {
        match buf[start - 1] {
            b' ' | b'\n' | b'\r' | b'\t' => break,
            _ => start -= 1,
        }
    }
    let token = &buf[start..end];
    // Comparer à "REJECT" insensible casse (no_std compatible)
    token.len() == 6
        && token.iter().zip(b"REJECT").all(|(a, b)| a.to_ascii_uppercase() == *b)
}

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let response_to_evaluate = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    let mut prompt: Vec<u8> = Vec::with_capacity(response_to_evaluate.len() + 192);
    prompt.extend_from_slice(
        b"Evaluate the following response. Think briefly, then end your reply \
with exactly one word on the last line: APPROVE if acceptable, REJECT if not.\n\
Response: "
    );
    prompt.extend_from_slice(response_to_evaluate);

    let mut resp_buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let resp_len = match infer(&prompt, &mut resp_buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, &[0x00]); // fail-open
            terminate();
            return;
        }
    };

    let approved = !last_token_is_reject(&resp_buf[..resp_len]);

    barrier();
    emit_raw(1, &[if approved { 0x00 } else { 0x01 }]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
