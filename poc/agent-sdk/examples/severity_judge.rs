// severity_judge.rs — agent WASM juge de revue de code.
//
// Reçoit un rapport de revue (texte avec [BLOCKER]/[WARNING]/[INFO]).
// Compte les sévérités, produit un verdict final.
// Émet le verdict comme ActionResult.
// Fin de réponse : "VERDICT: APPROVE" ou "VERDICT: REJECT"
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example severity_judge --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let review = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    let mut prompt: Vec<u8> = Vec::with_capacity(review.len() + 320);
    prompt.extend_from_slice(
        b"You are validating a code review report. \
Count exactly how many lines start with [BLOCKER], how many with [WARNING], \
how many with [INFO] in the report below. Do not re-evaluate the code yourself.\n\
State: \"BLOCKERs: N, WARNINGs: N, INFOs: N\"\n\
If BLOCKERs >= 1, end with exactly: VERDICT: REJECT\n\
Otherwise end with exactly: VERDICT: APPROVE\n\
\nReview report:\n"
    );
    prompt.extend_from_slice(review);

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[judge error: inference failed]");
            terminate();
            return;
        }
    };

    // Émettre le rapport complet du juge
    barrier();
    emit_raw(1, &buf[..n]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
