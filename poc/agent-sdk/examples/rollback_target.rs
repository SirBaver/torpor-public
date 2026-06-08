// rollback_target.rs — S4 : cible d'un rollback scheduler pendant une inférence en cours.
//
// Protocole (deux phases) :
//   Phase 0x01 — construit l'historique de snapshots (plusieurs appels autorisés).
//                barrier + emit ActionResult → snapshot enregistré dans ContentStore.
//   Phase 0x02 — lance une inférence longue (SleepyBackend 60s en test).
//                Si le scheduler rollback pendant l'inférence :
//                  → agent_infer retourne Err(INFER_CANCELLED=4)
//                  → l'agent termine proprement sans émettre de résultat
//
// Ce que démontre S4 :
//   - InferenceCancelled (0x0E) tracé dans le log quand le scheduler rollback pendant infer.
//   - SchedulerRollback (0x0B) dans le log avec caps_invalidated ≥ 1 (D8 ADR-0007).
//   - La cap accordée après le snapshot cible est révoquée post-rollback.
//   - Q5.1 ADR-0019 : rollback pendant WaitingInference → abort propre de la Future.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example rollback_target
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN, INFER_CANCELLED};

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 {
        terminate();
        return;
    }
    let p = ptr as *const u8;
    let cmd = p.read();
    match cmd {
        0x01 => phase_build_history(),
        0x02 => phase_long_inference(),
        _ => terminate(),
    }
}

unsafe fn phase_build_history() {
    // Construit un snapshot dans le ContentStore (cible du rollback S4).
    barrier();
    emit_raw(1, b"history:pre_rollback_target");
    // Ne termine pas — l'agent attend le message suivant.
}

unsafe fn phase_long_inference() {
    // Lance une inférence longue — en test, le scheduler rollback pendant cette attente.
    let mut resp_buf = [0u8; INFER_RESPONSE_BUF_LEN];
    match infer(b"Long computation - this will be cancelled.", &mut resp_buf, 60_000) {
        Ok(n) => {
            // Inférence complète (ne devrait pas arriver dans le scénario S4)
            barrier();
            emit_raw(1, &resp_buf[..n.min(128)]);
            terminate();
        }
        Err(code) if code == INFER_CANCELLED => {
            // Annulation propre par le scheduler (Q5.1 ADR-0019).
            // Ne PAS appeler terminate() : le run_loop doit rester actif pour
            // consommer le Message::Rollback qui suit immédiatement dans l'inbox.
            // Si on termine ici, le rollback ne sera jamais traité.
        }
        Err(_) => {
            barrier();
            emit_raw(1, b"infer_error");
            terminate();
        }
    }
}

#[allow(dead_code)]
fn main() {}
