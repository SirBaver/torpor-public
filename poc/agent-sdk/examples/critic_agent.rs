// critic_agent.rs — agent critique pour boucle itérative draft→review.
//
// Reçoit : contexte de la tâche + "\n---\n" + draft à évaluer.
// Évalue le draft sur les critères de la tâche.
// Première ligne : ACCEPT ou REVISE.
// Ensuite : feedback structuré (ce qui manque, ce qui doit changer).
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example critic_agent --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let input = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    // Séparer contexte et draft au séparateur "\n---\n"
    let sep = b"\n---\n";
    let (task_ctx, draft) = if let Some(pos) = input.windows(sep.len()).position(|w| w == sep) {
        (&input[..pos], &input[pos + sep.len()..])
    } else {
        (b"".as_slice(), input)
    };

    let mut prompt: Vec<u8> = Vec::with_capacity(input.len() + 512);
    prompt.extend_from_slice(
        b"You are a strict editor. Evaluate the draft against the task requirements.\n\
\n\
Your response MUST start with exactly ACCEPT or REVISE on the first line.\n\
- ACCEPT if the draft fully meets the requirements (quality, tone, completeness).\n\
- REVISE if anything is missing, weak, or off-target.\n\
\n\
If REVISE: list specific issues (1 per line, be concrete, not vague).\n\
If ACCEPT: one sentence explaining why it meets the bar.\n\
Do not rewrite the draft.\n\
\n\
Task requirements:\n"
    );
    if !task_ctx.is_empty() {
        prompt.extend_from_slice(task_ctx);
    }
    prompt.extend_from_slice(b"\n\nDraft to evaluate:\n");
    prompt.extend_from_slice(draft);

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"ACCEPT\n[critic error: fail-open]");
            terminate();
            return;
        }
    };

    barrier();
    emit_raw(1, &buf[..n]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
