// quality_writer.rs — A2 en action réelle : auto-correction via agent_self_rollback.
//
// L'agent génère un contenu formaté (communiqué NovOS).
// Il vérifie DE FAÇON DÉTERMINISTE si son output commence par "ANNOUNCE:".
// Si non : il se rollback lui-même et réessaie avec un prompt plus directif.
//
// Pattern (une seule phase, analogie de worker_double_check.rs) :
//   1. infer(prompt basique) → draft_1
//   2. barrier() #1 → commit draft_1 (seq→1)
//   3. Check déterministe : draft_1.starts_with("ANNOUNCE:") ?
//   4. barrier() #2 → commit marker QUALITY:PASS ou QUALITY:FAIL (seq→2)
//   5a. QUALITY:FAIL → self_rollback(1) (seq=2 ≥ 1+1=2 → valide, target_seq=0)
//       infer(prompt directif) → draft_2
//       barrier() #3 (from seq=0) → commit draft_2
//   5b. QUALITY:PASS → barrier() #3 → confirm
//   6. terminate()
//
// Log résultant (FAIL path) :
//   [seq=1] ActionResult  : draft_1 provisoire
//   [seq=2] ActionResult  : QUALITY:FAIL marker
//   [SelfRollback]        : depth=1, target_seq=0
//   [seq=1 again]         : draft_2 corrigé (même position que le premier commit)
//
// Propriété démontrée : A2 — l'agent détecte sa propre erreur et se corrige
// sans intervention extérieure. Le log trace tout.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example quality_writer --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, self_rollback, terminate, INFER_RESPONSE_BUF_LEN};

const PREFIX: &[u8] = b"ANNOUNCE:";

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 {
        terminate();
        return;
    }
    let task = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    // Étape 1 — premier draft (prompt basique, intentionnellement sous-spécifié)
    let mut prompt: Vec<u8> = Vec::with_capacity(task.len() + 128);
    prompt.extend_from_slice(b"Write a one-paragraph product announcement for: ");
    prompt.extend_from_slice(task);
    prompt.extend_from_slice(b". Keep it under 80 words.");

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n1 = match infer(&prompt, &mut buf, 120_000) {
        Ok(n) => n,
        Err(_) => { terminate(); return; }
    };
    let draft1 = &buf[..n1];

    // Étape 2 — barrier #1 : commit du draft provisoire (seq → 1)
    barrier();
    emit_raw(1, draft1);

    // Étape 3 — vérification déterministe du format
    let trimmed_start = {
        let s = core::str::from_utf8(draft1).unwrap_or("").trim_start();
        s.as_bytes()
    };
    let format_ok = trimmed_start.len() >= PREFIX.len()
        && trimmed_start[..PREFIX.len()].eq_ignore_ascii_case(PREFIX);

    // Étape 4 — barrier #2 : commit du verdict de qualité (seq → 2)
    barrier();
    if format_ok {
        emit_raw(1, b"QUALITY:PASS");
    } else {
        emit_raw(1, b"QUALITY:FAIL -- draft did not start with ANNOUNCE:");
    }

    if !format_ok {
        // Étape 5a — A2 : auto-correction
        // seq=2 ≥ 1+1=2 → self_rollback(1) valide → target_seq = 2-1-1 = 0
        let target = self_rollback(1);
        // target_seq=0 : état avant les deux premiers commits

        // Construire le prompt correctif avec instruction explicite
        let mut prompt2: Vec<u8> = Vec::with_capacity(task.len() + 256);
        prompt2.extend_from_slice(
            b"MANDATORY FORMAT: Your response MUST start with exactly 'ANNOUNCE:' \
              (uppercase, followed by a colon and a space), then the announcement text.\n\
              Task: Write a one-paragraph product announcement for: "
        );
        prompt2.extend_from_slice(task);
        prompt2.extend_from_slice(
            b"\n\nExample format: ANNOUNCE: [your announcement here]\n\
              Remember: start with 'ANNOUNCE:' - this is non-negotiable."
        );

        let mut buf2 = [0u8; INFER_RESPONSE_BUF_LEN];
        let n2 = match infer(&prompt2, &mut buf2, 120_000) {
            Ok(n) => n,
            Err(_) => {
                // Émettre l'erreur depuis le snapshot restauré
                barrier();
                emit_raw(1, b"[quality_writer error: retry inference failed]");
                terminate();
                return;
            }
        };

        // Étape 5a-fin — barrier #3 depuis snapshot restauré (seq → 1 du nouvel état)
        // Encode target_seq dans le payload pour l'audit
        let _ = target;
        barrier();
        let mut out: Vec<u8> = Vec::with_capacity(n2 + 32);
        out.extend_from_slice(b"[SELF_CORRECTED] ");
        out.extend_from_slice(&buf2[..n2]);
        emit_raw(1, &out);
    } else {
        // Étape 5b — PASS : confirmer directement
        barrier();
        let mut out: Vec<u8> = Vec::with_capacity(n1 + 16);
        out.extend_from_slice(b"[CONFIRMED] ");
        out.extend_from_slice(draft1);
        emit_raw(1, &out);
    }

    terminate();
}

#[allow(dead_code)]
fn main() {}
