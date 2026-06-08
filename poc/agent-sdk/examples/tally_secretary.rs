// tally_secretary.rs — secrétaire de vote WASM déterministe (ADR-0054).
//
// Reçoit les votes (un par Message::Data) puis un Message::Data("TALLY:<N>").
// Calcule la majorité simple : votes_pour > N / 2 (D1, ADR-0054).
// Vote manquant = abstention (dénominateur N, D2).
// Émet ActionResult : payload MessagePack {decision, approve, reject, abstain, n}.
// Les causes (action_ids des votes reçus) sont posées par le runner via caused_by.
//
// AUCUN appel d'inférence — code pur uniquement.
//
// Protocole :
//   1. Runner spawne le secrétaire.
//   2. Pour chaque vote reçu :
//      runner envoie Message::Data(b"VOTE:APPROVE") ou Message::Data(b"VOTE:REJECT")
//      (le runner a déjà parsé le texte LLM, ne fait que transmettre le verdict)
//   3. Runner envoie Message::Data(b"TALLY:<N>") avec N = nombre de voters attendus.
//   4. Secrétaire calcule, émet ActionResult, termine.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example tally_secretary --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, terminate};

static mut N_APPROVE: u32 = 0;
static mut N_REJECT: u32 = 0;
static mut N_RECEIVED: u32 = 0;

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let input = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    if input.starts_with(b"VOTE:APPROVE") {
        N_APPROVE += 1;
        N_RECEIVED += 1;
    } else if input.starts_with(b"VOTE:REJECT") {
        N_REJECT += 1;
        N_RECEIVED += 1;
    } else if input.starts_with(b"TALLY:") {
        // Extraire N (voters attendus)
        let n_str = &input[6..];
        let n_expected: u32 = n_str.iter().take_while(|&&b| b >= b'0' && b <= b'9')
            .fold(0u32, |acc, &b| acc * 10 + (b - b'0') as u32);

        let n_abstain = n_expected.saturating_sub(N_RECEIVED);
        // Majorité simple : votes_pour > N_expected / 2 (ADR-0054 D1)
        let approved = N_APPROVE * 2 > n_expected;
        let decision = if approved { b"APPROVED" as &[u8] } else { b"REJECTED" };

        // Payload lisible : "DECISION:<APPROVED|REJECTED> APPROVE:<n> REJECT:<n> ABSTAIN:<n> N:<n>"
        let mut out: [u8; 128] = [0u8; 128];
        let mut pos = 0;

        let write_field = |buf: &mut [u8; 128], p: &mut usize, label: &[u8], val: u32| {
            for &b in label { buf[*p] = b; *p += 1; }
            let s = val.to_string();
            for b in s.bytes() { buf[*p] = b; *p += 1; }
            buf[*p] = b' '; *p += 1;
        };

        for &b in b"DECISION:" { out[pos] = b; pos += 1; }
        for &b in decision { out[pos] = b; pos += 1; }
        out[pos] = b' '; pos += 1;
        write_field(&mut out, &mut pos, b"APPROVE:", N_APPROVE);
        write_field(&mut out, &mut pos, b"REJECT:", N_REJECT);
        write_field(&mut out, &mut pos, b"ABSTAIN:", n_abstain);
        write_field(&mut out, &mut pos, b"N:", n_expected);

        barrier();
        emit_raw(1, &out[..pos.saturating_sub(1)]); // trim trailing space
        terminate();
    }
    // Autres messages ignorés silencieusement (robustesse)
}

#[allow(dead_code)]
fn main() {}
