// orchestrator.rs — agent qui délègue à un spécialiste via Event (ADR-0010).
//
// Phase 0x01 [question] :
//   - appelle agent_infer pour analyser la question
//   - émet Event (0x03) avec payload "delegate:<question>" pour signaler au runner
//   - retourne sans terminer (attend la réponse du spécialiste)
//
// Phase 0x02 [reponse_specialiste] :
//   - reçoit la réponse du spécialiste (injectée par le runner)
//   - synthétise en combinant sa propre analyse + la réponse du spécialiste
//   - émet ActionResult (0x01) final
//   - terminate
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example orchestrator --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

static mut ANALYSIS: [u8; INFER_RESPONSE_BUF_LEN] = [0u8; INFER_RESPONSE_BUF_LEN];
static mut ANALYSIS_LEN: usize = 0;

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let p = ptr as *const u8;
    match p.read() {
        0x01 => phase_analyze(p.add(1), (len - 1) as usize),
        0x02 => phase_synthesize(p.add(1), (len - 1) as usize),
        _    => { terminate(); }
    }
}

unsafe fn phase_analyze(ptr: *const u8, len: usize) {
    let question = core::slice::from_raw_parts(ptr, len);

    // Analyse initiale de la question
    let mut prompt: Vec<u8> = Vec::with_capacity(question.len() + 64);
    prompt.extend_from_slice(b"Analyse brievement (1 phrase) : ");
    prompt.extend_from_slice(question);

    let n = match infer(&prompt, &mut ANALYSIS, 180_000) {
        Ok(n) => n,
        Err(_) => {
            // En cas d'échec, déléguer quand même
            let fallback = b"analyse indisponible";
            ANALYSIS[..fallback.len()].copy_from_slice(fallback);
            fallback.len()
        }
    };
    ANALYSIS_LEN = n;

    // Émettre le signal de délégation : Event (0x03) avec payload "delegate:<question>"
    let mut event_payload: Vec<u8> = Vec::with_capacity(question.len() + 9);
    event_payload.extend_from_slice(b"delegate:");
    event_payload.extend_from_slice(question);

    barrier();
    emit_raw(0x03, &event_payload); // EmitType::Event = 0x03
    // Ne pas terminer — on attend Message::Data avec la réponse du spécialiste (phase 0x02)
}

unsafe fn phase_synthesize(ptr: *const u8, len: usize) {
    let specialist_answer = core::slice::from_raw_parts(ptr, len);

    // Synthèse : analyse propre + réponse spécialiste
    let mut prompt: Vec<u8> = Vec::with_capacity(ANALYSIS_LEN + specialist_answer.len() + 128);
    prompt.extend_from_slice(b"Combine en 1 phrase ces deux informations. Info1: ");
    prompt.extend_from_slice(&ANALYSIS[..ANALYSIS_LEN]);
    prompt.extend_from_slice(b". Info2: ");
    prompt.extend_from_slice(specialist_answer);

    let mut resp_buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut resp_buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            // Fallback : retourner la réponse du spécialiste directement
            let l = specialist_answer.len().min(resp_buf.len());
            resp_buf[..l].copy_from_slice(&specialist_answer[..l]);
            l
        }
    };

    barrier();
    emit_raw(1, &resp_buf[..n]); // EmitType::ActionResult = 1
    terminate();
}

#[allow(dead_code)]
fn main() {}
