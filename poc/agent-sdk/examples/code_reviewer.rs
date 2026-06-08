// code_reviewer.rs — agent WASM de revue de code.
//
// Reçoit un snippet de code en bytes.
// Appelle infer avec un prompt de reviewer.
// Émet le rapport de revue comme ActionResult.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example code_reviewer --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let code = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    let mut prompt: Vec<u8> = Vec::with_capacity(code.len() + 320);
    prompt.extend_from_slice(
        b"You are a security-focused code reviewer. List ONLY the issues you find, \
one per line, starting with the severity tag. Do NOT repeat or quote the code.\n\
\nSeverity rules (apply strictly):\n\
  [BLOCKER] = must fix before merge: SQL injection, hardcoded secrets, \
missing authentication, missing authorization, unsafe deserialization, \
plaintext passwords, unvalidated user input used in queries or shell commands.\n\
  [WARNING] = should fix: missing error handling, race conditions, \
missing input validation, insecure defaults.\n\
  [INFO]    = nice to fix: style issues, missing comments, dead code.\n\
\nFormat: [SEVERITY] <function_name>: <one-line description>\n\
Do not write anything else. Do not repeat the code.\n\
\nCode to review:\n"
    );
    prompt.extend_from_slice(code);

    let mut buf = [0u8; INFER_RESPONSE_BUF_LEN];
    let n = match infer(&prompt, &mut buf, 180_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[review error: inference failed]");
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
