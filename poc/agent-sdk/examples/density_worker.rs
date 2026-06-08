// density_worker.rs — S3 : worker minimal pour démontrer la borne dure du pool d'inférence.
//
// Reçoit n'importe quel message, appelle agent_infer une fois, émet le résultat, termine.
// N instances lancées simultanément avec InferencePool(cap=4) : au plus 4 inférences
// s'exécutent en parallèle ; les autres attendent un slot (LifecycleState::WaitingInference).
//
// Ce que démontre S3 :
//   - La borne dure sur les inférences concurrentes (k=4) via sémaphore Tokio.
//   - L'état WaitingInference est observable dans le log causal.
//   - Pas de famine définitive : tous les workers finissent par compléter.
//
// Ce que S3 NE démontre PAS : équité, priorité, borne sur la latence d'attente.
// Ces propriétés fortes de C1 restent du ressort de Phase 6.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example density_worker
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

#[no_mangle]
pub unsafe extern "C" fn process(_ptr: i32, _len: i32) {
    let mut resp_buf = [0u8; INFER_RESPONSE_BUF_LEN];
    match infer(b"Count from 1 to 3.", &mut resp_buf, 60_000) {
        Ok(n) => {
            barrier();
            // Émettre les 64 premiers bytes de la réponse (ou moins si plus court)
            let out_len = n.min(64);
            emit_raw(1, &resp_buf[..out_len]);
        }
        Err(_) => {
            barrier();
            emit_raw(1, b"infer_error");
        }
    }
    terminate();
}

#[allow(dead_code)]
fn main() {}
