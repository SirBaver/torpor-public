// incident_aggregator.rs — agent fan-in : accumule les rapports partiels, synthétise.
//
// Protocole :
//   REPORT:infra:<texte>     → stocker rapport infrastructure
//   REPORT:db:<texte>        → stocker rapport base de données
//   REPORT:security:<texte>  → stocker rapport sécurité
//   FINALIZE                 → appeler infer, émettre ActionResult, terminer
//
// Pas de terminate avant FINALIZE — accumule les N rapports dans la RAM WASM.
// Le runner envoie les REPORT via Message::caused (liés aux ActionResult spécialistes).
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example incident_aggregator --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

const BUF: usize = 2048;
static mut INFRA:     [u8; BUF] = [0u8; BUF];
static mut INFRA_LEN: usize = 0;
static mut DB:        [u8; BUF] = [0u8; BUF];
static mut DB_LEN:    usize = 0;
static mut SECURITY:  [u8; BUF] = [0u8; BUF];
static mut SEC_LEN:   usize = 0;

unsafe fn store(dst: *mut u8, dst_len: *mut usize, src: &[u8]) {
    let n = src.len().min(BUF);
    core::ptr::copy_nonoverlapping(src.as_ptr(), dst, n);
    *dst_len = n;
}

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let input = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    if input.starts_with(b"REPORT:infra:") {
        store(INFRA.as_mut_ptr(), &raw mut INFRA_LEN, &input[13..]);
    } else if input.starts_with(b"REPORT:db:") {
        store(DB.as_mut_ptr(), &raw mut DB_LEN, &input[10..]);
    } else if input.starts_with(b"REPORT:security:") {
        store(SECURITY.as_mut_ptr(), &raw mut SEC_LEN, &input[16..]);
    } else if input.starts_with(b"FINALIZE") {
        let mut prompt: Vec<u8> = Vec::with_capacity(INFRA_LEN + DB_LEN + SEC_LEN + 512);
        prompt.extend_from_slice(
            b"You are an incident commander. Three specialists have analyzed a production incident.\n\
Synthesize their findings into a concise incident report:\n\
1. Most likely root cause (1 sentence)\n\
2. Immediate actions (3 bullets)\n\
3. Escalation needed? Yes/No and why.\n\n"
        );
        if INFRA_LEN > 0 {
            prompt.extend_from_slice(b"[Infrastructure analysis]\n");
            prompt.extend_from_slice(core::slice::from_raw_parts(INFRA.as_ptr(), INFRA_LEN));
            prompt.extend_from_slice(b"\n\n");
        }
        if DB_LEN > 0 {
            prompt.extend_from_slice(b"[Database analysis]\n");
            prompt.extend_from_slice(core::slice::from_raw_parts(DB.as_ptr(), DB_LEN));
            prompt.extend_from_slice(b"\n\n");
        }
        if SEC_LEN > 0 {
            prompt.extend_from_slice(b"[Security analysis]\n");
            prompt.extend_from_slice(core::slice::from_raw_parts(SECURITY.as_ptr(), SEC_LEN));
            prompt.extend_from_slice(b"\n\n");
        }

        let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
        let n = match infer(&prompt, &mut buf, 180_000) {
            Ok(n) => n,
            Err(_) => {
                barrier();
                emit_raw(1, b"[aggregator error: inference failed]");
                terminate();
                return;
            }
        };
        barrier();
        emit_raw(1, &buf[..n]);
        terminate();
    }
    // Autres messages ignorés
}

#[allow(dead_code)]
fn main() {}
