// monitor_agent.rs
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

const REPORT_BUF: usize = 1024;

static mut REPORT_A:     [u8; REPORT_BUF] = [0u8; REPORT_BUF];
static mut REPORT_A_LEN: usize = 0;
static mut SEEN_A:       bool  = false;
static mut SYNTH:        [u8; INFER_RESPONSE_BUF_LEN] = [0u8; INFER_RESPONSE_BUF_LEN];

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let data = core::slice::from_raw_parts(ptr as *const u8, len as usize);
    let seen = *core::ptr::addr_of!(SEEN_A);
    if !seen {
        let copy_len = data.len().min(REPORT_BUF);
        let buf = &mut *core::ptr::addr_of_mut!(REPORT_A);
        buf[..copy_len].copy_from_slice(&data[..copy_len]);
        *core::ptr::addr_of_mut!(REPORT_A_LEN) = copy_len;
        *core::ptr::addr_of_mut!(SEEN_A) = true;
        return;
    }
    let r_a = &*core::ptr::addr_of!(REPORT_A);
    let l_a = *core::ptr::addr_of!(REPORT_A_LEN);
    let mut prompt: Vec<u8> = Vec::with_capacity(512 + l_a + data.len());
    prompt.extend_from_slice(b"You are a supervisor monitoring two AI agents.
");
    prompt.extend_from_slice(b"Write a SUPERVISION REPORT in 3 sentences.
Start with: SUPERVISION REPORT:

");
    prompt.extend_from_slice(b"AGENT A OUTPUT:
");
    prompt.extend_from_slice(&r_a[..l_a]);
    prompt.extend_from_slice(b"

AGENT B OUTPUT:
");
    prompt.extend_from_slice(data);
    prompt.extend_from_slice(b"

SUPERVISION REPORT:");
    let synth = &mut *core::ptr::addr_of_mut!(SYNTH);
    let resp_len = match infer(&prompt, synth, 90_000) {
        Ok(n) => n,
        Err(_) => { barrier(); emit_raw(1, b"[error]"); terminate(); return; }
    };
    barrier();
    emit_raw(1, &synth[..resp_len]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
