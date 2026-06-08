// task_step.rs — agent WASM stateless pour tâche multi-étapes.
//
// Reçoit : contexte des étapes précédentes + instruction courante (séparés par "\n---\n").
// Produit : résultat de cette étape en 2-3 phrases. Termine.
//
// Stateless par conception : tout l'état persistant vient du log causal via le runner.
// Le runner lit les étapes précédentes depuis le log et les injecte dans ce message.
// C'est la propriété P1a : la RAM WASM est un cache volatile, le log est autoritaire.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example task_step --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let input = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    // Séparer contexte et instruction au séparateur "\n---\n"
    let sep = b"\n---\n";
    let (context, instruction) = if let Some(pos) = input.windows(sep.len()).position(|w| w == sep) {
        (&input[..pos], &input[pos + sep.len()..])
    } else {
        (b"".as_slice(), input)
    };

    let mut prompt: Vec<u8> = Vec::with_capacity(input.len() + 256);
    prompt.extend_from_slice(b"You are working step by step on a task. Be concise (2-3 sentences max).\n");

    if !context.is_empty() {
        prompt.extend_from_slice(b"\nPrevious steps completed:\n");
        prompt.extend_from_slice(context);
        prompt.extend_from_slice(b"\n");
    }

    prompt.extend_from_slice(b"\nYour task for this step:\n");
    prompt.extend_from_slice(instruction);

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[step error: inference failed]");
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
