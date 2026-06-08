// voter_agent.rs — agent votant pour décision collective.
//
// Reçoit une proposition. Émet un vote indépendant : APPROVE ou REJECT.
// Première ligne = verdict, reste = justification courte.
// Chaque instance est indépendante — pas de communication inter-agents.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example voter_agent --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let proposal = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    let mut prompt: Vec<u8> = Vec::with_capacity(proposal.len() + 384);
    prompt.extend_from_slice(
        b"You are a technical reviewer voting on a proposal. \
Give an independent assessment.\n\
\n\
Your response MUST start with exactly APPROVE or REJECT on the first line.\n\
Then give 1-2 sentences of justification.\n\
Do not hedge or qualify your vote - pick one.\n\
\n\
Proposal:\n"
    );
    prompt.extend_from_slice(proposal);

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"REJECT\n[inference error]");
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
