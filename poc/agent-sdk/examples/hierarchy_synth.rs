// hierarchy_synth.rs -- synthétiseur de délégation hiérarchique (use case M).
//
// Reçoit 3 rapports sans appeler barrier() sur les 2 premiers :
//   msg 0 : brief manager      (cause = action_id_manager)
//   msg 1 : rapport sécurité   (cause = action_id_analyst_sec)
//   msg 2 : rapport performance (cause = action_id_analyst_perf)
// Au 3e : barrier() = fan-in DAG, infer synthèse, terminate.
//
// Build:
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example hierarchy_synth --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

const REPORT_BUF: usize = 768;

static mut REPORTS:     [[u8; REPORT_BUF]; 3] = [[0u8; REPORT_BUF]; 3];
static mut REPORT_LENS: [usize; 3]            = [0usize; 3];
static mut COUNT:       usize                 = 0;
static mut SYNTH:       [u8; INFER_RESPONSE_BUF_LEN] = [0u8; INFER_RESPONSE_BUF_LEN];

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let data = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    let idx = *core::ptr::addr_of!(COUNT);
    if idx >= 3 { terminate(); return; }

    let copy_len = data.len().min(REPORT_BUF);
    let reports = &mut *core::ptr::addr_of_mut!(REPORTS);
    reports[idx][..copy_len].copy_from_slice(&data[..copy_len]);
    let lens = &mut *core::ptr::addr_of_mut!(REPORT_LENS);
    lens[idx] = copy_len;
    *core::ptr::addr_of_mut!(COUNT) = idx + 1;

    if idx + 1 < 3 { return; }

    let r = &*core::ptr::addr_of!(REPORTS);
    let l = &*core::ptr::addr_of!(REPORT_LENS);

    let mut prompt: Vec<u8> = Vec::with_capacity(512 + l[0] + l[1] + l[2]);
    prompt.extend_from_slice(
        b"You are a chief architect synthesizing inputs from a 2-level agent hierarchy.
        Produce a final architecture RECOMMENDATION in 3-4 sentences.
        Start your response with: RECOMMENDATION:

",
    );
    prompt.extend_from_slice(b"MANAGER REQUIREMENTS:
");
    prompt.extend_from_slice(&r[0][..l[0]]);
    prompt.extend_from_slice(b"

SECURITY ANALYST REPORT:
");
    prompt.extend_from_slice(&r[1][..l[1]]);
    prompt.extend_from_slice(b"

PERFORMANCE ANALYST REPORT:
");
    prompt.extend_from_slice(&r[2][..l[2]]);
    prompt.extend_from_slice(b"

RECOMMENDATION:");

    let synth = &mut *core::ptr::addr_of_mut!(SYNTH);
    let resp_len = match infer(&prompt, synth, 120_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[hierarchy synthesis error]");
            terminate();
            return;
        }
    };

    // barrier() -- parent_ids = [last_action_synth, mgr_id, sec_id, perf_id] = fan-in
    barrier();
    emit_raw(1, &synth[..resp_len]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
