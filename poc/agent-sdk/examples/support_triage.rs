// support_triage.rs — agent support client niveau 1 avec routing dynamique.
//
// Reçoit une question client.
// Décide : répondre directement (ANSWER) ou escalader vers un spécialiste (ESCALATE).
// Si ANSWER  : émet ActionResult et termine.
// Si ESCALATE: émet Event("escalate:<type>:<question>"), attend la réponse du spécialiste.
//
// Phase 0x02 [réponse spécialiste] :
//   Synthétise et émet ActionResult final. Termine.
//
// La décision de routing est committée dans le log (Event ou ActionResult direct) —
// auditée, rejouable, causalement liée à la réponse finale.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example support_triage --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

const QUESTION_BUF: usize = 2048;
static mut QUESTION: [u8; QUESTION_BUF] = [0u8; QUESTION_BUF];
static mut QUESTION_LEN: usize = 0;

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let p = ptr as *const u8;
    if len > 1 && p.read() == 0x02 {
        phase_synthesize(p.add(1), (len - 1) as usize);
    } else {
        phase_triage(p, len as usize);
    }
}

unsafe fn phase_triage(ptr: *const u8, len: usize) {
    let question = core::slice::from_raw_parts(ptr, len);

    // Sauvegarder la question pour phase_synthesize
    let qlen = question.len().min(QUESTION_BUF);
    QUESTION[..qlen].copy_from_slice(&question[..qlen]);
    QUESTION_LEN = qlen;

    let mut prompt: Vec<u8> = Vec::with_capacity(question.len() + 512);
    prompt.extend_from_slice(
        b"You are a Level 1 customer support agent. Route the customer question.\n\
\n\
Your response MUST start with ANSWER: or ESCALATE: (no other format accepted).\n\
\n\
ANSWER: for simple questions you can answer directly.\n\
  Examples: business hours, how to reset password, how to create account, general info.\n\
\n\
ESCALATE: for issues requiring a specialist. Choose the specialist type:\n\
  technical  -> system crashes, data loss, bugs, API errors, security incidents\n\
  billing    -> invoices, payment failures, refunds, subscription charges\n\
  sales      -> enterprise contracts, pricing negotiation, volume discounts, partnerships\n\
\n\
Format if ANSWER:   ANSWER: <your 1-2 sentence response>\n\
Format if ESCALATE: ESCALATE: <technical|billing|sales>: <one-line reason>\n\
\n\
Customer question: "
    );
    prompt.extend_from_slice(question);

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[triage error]");
            terminate();
            return;
        }
    };

    let response = &buf[..n];

    // Parser la décision de routing
    let upper_prefix: [u8; 9] = {
        let mut a = [0u8; 9];
        for (i, b) in response.iter().take(9).enumerate() {
            a[i] = b.to_ascii_uppercase();
        }
        a
    };

    if upper_prefix.starts_with(b"ESCALATE:") {
        // Émettre un Event de délégation — tracé dans le log
        let mut event: Vec<u8> = Vec::with_capacity(question.len() + response.len() + 10);
        event.extend_from_slice(b"escalate:");
        // Inclure le type de spécialiste depuis la réponse
        let after_prefix = response.iter().skip(9)
            .position(|&b| b != b' ')
            .map(|i| &response[9 + i..])
            .unwrap_or(response);
        event.extend_from_slice(after_prefix);

        barrier();
        emit_raw(0x03, &event); // EmitType::Event = 0x03
        // Ne pas terminer — on attend la réponse du spécialiste (phase 0x02)
    } else {
        // ANSWER ou fallback : répondre directement
        let answer = if upper_prefix.starts_with(b"ANSWER:") {
            response.iter().skip(7)
                .position(|&b| b != b' ')
                .map(|i| &response[7 + i..])
                .unwrap_or(response)
        } else {
            response
        };
        barrier();
        emit_raw(1, answer); // ActionResult direct
        terminate();
    }
}

unsafe fn phase_synthesize(ptr: *const u8, len: usize) {
    let specialist_answer = core::slice::from_raw_parts(ptr, len);

    let mut prompt: Vec<u8> = Vec::with_capacity(QUESTION_LEN + specialist_answer.len() + 256);
    prompt.extend_from_slice(
        b"You are a customer support agent. A specialist has answered the customer's question.\n\
Write a professional, concise response (2-3 sentences) to the customer.\n\
\nOriginal question: "
    );
    prompt.extend_from_slice(&QUESTION[..QUESTION_LEN]);
    prompt.extend_from_slice(b"\nSpecialist answer: ");
    prompt.extend_from_slice(specialist_answer);

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            // Fallback : retourner la réponse du spécialiste directement
            let l = specialist_answer.len().min(buf.len());
            buf[..l].copy_from_slice(&specialist_answer[..l]);
            l
        }
    };

    barrier();
    emit_raw(1, &buf[..n]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
