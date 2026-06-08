// llm_worker.rs — agent worker qui génère une réponse et demande validation.
//
// Protocole deux phases :
//   Phase 0x01 [payload: question bytes] :
//     appelle agent_infer, émet la réponse provisoire (ActionResult),
//     puis request_validation(1). Retourne sans terminer.
//   Phase 0x02 :
//     lit le verdict (0=Approuvé, 1=Rejeté, 2=Timeout),
//     émet le résultat final (ActionResult), terminate.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example llm_worker --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, get_verdict, infer, request_validation, terminate, INFER_RESPONSE_BUF_LEN};

static mut PROVISIONAL: [u8; INFER_RESPONSE_BUF_LEN] = [0u8; INFER_RESPONSE_BUF_LEN];
static mut PROVISIONAL_LEN: usize = 0;

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let p = ptr as *const u8;
    match p.read() {
        0x01 => phase_generate(p.add(1), (len - 1) as usize),
        0x02 => phase_apply_verdict(),
        _    => { terminate(); }
    }
}

unsafe fn phase_generate(ptr: *const u8, len: usize) {
    let question = core::slice::from_raw_parts(ptr, len);

    // Construire le prompt
    let mut prompt: Vec<u8> = Vec::with_capacity(question.len() + 32);
    prompt.extend_from_slice(b"Reponds en 1-2 phrases: ");
    prompt.extend_from_slice(question);

    let resp_len = match infer(&prompt, &mut PROVISIONAL, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[inference error]");
            terminate();
            return;
        }
    };
    PROVISIONAL_LEN = resp_len;

    // Émettre la réponse provisoire
    barrier();
    emit_raw(1, &PROVISIONAL[..resp_len]);

    // Demander validation au superviseur (risque = 1 = medium)
    request_validation(1);
    // Ne pas terminer — on attend Message::ValidationResponse → phase 0x02
}

unsafe fn phase_apply_verdict() {
    let verdict = get_verdict(); // 0=Approved 1=Rejected 2=Timeout
    barrier();
    match verdict {
        0 => {
            // Approuvé : émettre la réponse finale identique à la provisoire
            emit_raw(1, &PROVISIONAL[..PROVISIONAL_LEN]);
        }
        1 => {
            emit_raw(1, b"[reponse rejetee par superviseur]");
        }
        _ => {
            emit_raw(1, b"[validation timeout]");
        }
    }
    terminate();
}

#[allow(dead_code)]
fn main() {}
