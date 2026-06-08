// approval_agent.rs — A3 en action réelle : agent qui propose un plan risqué
// et attend approbation avant de l'exécuter.
//
// Protocole deux phases (identique à llm_worker.rs) :
//   Phase 0x01 [payload: task bytes] :
//     appelle infer pour générer un plan d'action,
//     émet le plan provisoire (ActionResult),
//     appelle request_validation(2) — risque HIGH.
//     Retourne sans terminer — l'agent attend Message::ValidationResponse.
//
//   Phase 0x02 :
//     lit le verdict via get_verdict() (0=Approved, 1=Rejected, 2=Timeout),
//     émet le résultat final avec le label correspondant,
//     terminate.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example approval_agent --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{
    barrier, emit_raw, get_verdict, infer, request_validation, terminate,
    INFER_RESPONSE_BUF_LEN,
};

static mut PLAN: [u8; INFER_RESPONSE_BUF_LEN] = [0u8; INFER_RESPONSE_BUF_LEN];
static mut PLAN_LEN: usize = 0;

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let p = ptr as *const u8;
    match p.read() {
        0x01 => phase_plan(p.add(1), (len - 1) as usize),
        0x02 => phase_finalize(),
        _    => { terminate(); }
    }
}

unsafe fn phase_plan(ptr: *const u8, len: usize) {
    let task = core::slice::from_raw_parts(ptr, len);

    let mut prompt: Vec<u8> = Vec::with_capacity(task.len() + 256);
    prompt.extend_from_slice(
        b"You are a database administrator. Generate a concrete action plan \
for the following database maintenance task. List each operation as a \
numbered step (e.g. '1. DROP TABLE ...'). Be specific.\n\nTask: "
    );
    prompt.extend_from_slice(task);

    let resp_len = match infer(&prompt, unsafe { &mut *core::ptr::addr_of_mut!(PLAN) }, 120_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[inference error]");
            terminate();
            return;
        }
    };
    PLAN_LEN = resp_len;

    // Émettre le plan provisoire
    barrier();
    emit_raw(1, &PLAN[..PLAN_LEN]);

    // A3 : demander validation — risk=2 (HIGH, opération irréversible)
    request_validation(2);
    // Ne pas terminer — on attend ValidationResponse puis phase 0x02
}

unsafe fn phase_finalize() {
    let verdict = get_verdict(); // 0=Approved 1=Rejected 2=Timeout
    barrier();
    match verdict {
        0 => {
            // Approuvé : plan validé, exécution autorisée
            let mut out: Vec<u8> = b"APPROVED -- EXECUTING PLAN:\n".to_vec();
            out.extend_from_slice(&PLAN[..PLAN_LEN]);
            emit_raw(1, &out);
        }
        1 => {
            // Rejeté : plan bloqué par le superviseur
            let mut out: Vec<u8> = b"REJECTED -- PLAN BLOCKED BY SUPERVISOR:\n".to_vec();
            out.extend_from_slice(&PLAN[..PLAN_LEN]);
            emit_raw(1, &out);
        }
        _ => {
            emit_raw(1, b"TIMEOUT -- validation timed out, operation aborted");
        }
    }
    terminate();
}

#[allow(dead_code)]
fn main() {}
