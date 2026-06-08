//! Jalon C.7-crash — Runtime process (instrumented avec kill points)
//!
//! Charge le module WASM agent.cwasm (pré-compilé AOT).
//! Fournit la host function emit() qui implémente la séquence Q3-C
//! avec kill points (KP1–KP4) pour tester I3-N et I4.
//!
//! KILL_POINT est une constante de compilation (env var KILL_POINT=0|1|2|3|4).
//!   KP0 (par défaut) : comportement nominal sans suspension
//!   KP1 : suspend après push blob, avant push header
//!   KP2 : suspend après push header, avant push log_entry
//!   KP3 : suspend après write log_entry + fence + data_len, avant ep.call()
//!   KP4 : suspend après retour de ep.call() (commit complet)
//!
//! Cap layout runtime CNode (size_bits=2 → 4 slots):
//!   Slot 0: NULL
//!   Slot 1: endpoint (CapRights::all()) — pour ep.call() (badge = agent_id)
//!   Slot 2: suspend_nfn (write_only) — signal au superviseur avant self-suspend
//!   Slot 3: own TCB cap (CapRights::all())

#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::Ordering;

use sel4::CapTypeForFrameObjectOfFixedSize;
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Linker, Module, Store};

mod platform;
mod runtime;

// Cap layout dans le runtime CNode (child_cnode_size_bits = 2)
// Slot 0: NULL
// Slot 1: endpoint (read_write + grant_reply, badge = agent_id)
// Slot 2: suspend_nfn (write_only) — signal au superviseur avant self-suspend
// Slot 3: own TCB cap
const EP_SLOT: u64 = 1;
const SUSPEND_NFN_SLOT: u64 = 2;
const TCB_SLOT: u64 = 3;

// Constante de compilation — env var KILL_POINT=0|1|2|3|4
// Si non définie ou invalide : 0 (comportement nominal)
// Note: match sur &str en const context non supporté dans nightly-2026-03-18 (L72)
const fn parse_kill_point(s: Option<&'static str>) -> u32 {
    match s {
        None => 0,
        Some(v) => {
            let b = v.as_bytes();
            if b.len() == 1 && b[0] == b'1' { 1 }
            else if b.len() == 1 && b[0] == b'2' { 2 }
            else if b.len() == 1 && b[0] == b'3' { 3 }
            else if b.len() == 1 && b[0] == b'4' { 4 }
            else { 0 }
        }
    }
}
const KILL_POINT: u32 = parse_kill_point(option_env!("KILL_POINT"));

// Module WASM pré-compilé par build.rs
static AGENT_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent.cwasm"));

// Payload distinct de C.6-crash pour des hashes différents
static EMIT_PAYLOAD: &[u8] = b"C7_CRASH_LOG_ENTRY_FROM_AGENT";

// Ring buffer layout (partagé avec le serveur)
#[repr(C)]
struct RingBuffer {
    data_len: u32,
    data: [u8; 4092],
}

fn get_ring_va() -> *mut RingBuffer {
    unsafe extern "C" {
        static _end: usize;
    }
    let ipc_buf = (core::ptr::addr_of!(_end) as usize)
        .next_multiple_of(sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes());
    let ring = ipc_buf + sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes();
    ring as *mut RingBuffer
}

fn write_record(buf: &mut [u8], pos: &mut usize, kind: u8, hash: &[u8; 32], payload: &[u8]) -> bool {
    let needed = 1 + 32 + 4 + payload.len();
    if *pos + needed > buf.len() {
        return false;
    }
    buf[*pos] = kind;
    *pos += 1;
    buf[*pos..*pos + 32].copy_from_slice(hash);
    *pos += 32;
    let plen = payload.len() as u32;
    buf[*pos..*pos + 4].copy_from_slice(&plen.to_le_bytes());
    *pos += 4;
    buf[*pos..*pos + payload.len()].copy_from_slice(payload);
    *pos += payload.len();
    true
}

fn self_suspend_at(kp: u32) -> ! {
    sel4::debug_println!("[C7-crash] runtime: self-suspend KP{}", kp);
    let suspend_nfn = sel4::cap::Notification::from_bits(SUSPEND_NFN_SLOT);
    suspend_nfn.signal();
    sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
    unreachable!()
}

struct EmitCtx {
    ep: sel4::cap::Endpoint,
    ring_va: *mut RingBuffer,
}

unsafe impl Send for EmitCtx {}
unsafe impl Sync for EmitCtx {}

pub fn main() -> ! {
    sel4::debug_println!("[C7-crash] runtime: démarrage (KILL_POINT={})", KILL_POINT);

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let suspend_nfn = sel4::cap::Notification::from_bits(SUSPEND_NFN_SLOT);
    let ring_va = get_ring_va();

    unsafe {
        (*ring_va).data_len = 0;
    }

    let ctx = EmitCtx { ep, ring_va };

    let engine = Engine::default();
    let module = unsafe {
        Module::deserialize(&engine, AGENT_CWASM).expect("Module::deserialize échoué")
    };

    let mut linker: Linker<EmitCtx> = Linker::new(&engine);

    linker
        .func_wrap("env", "emit", |caller: wasmtime::Caller<'_, EmitCtx>| -> i32 {
            let payload = EMIT_PAYLOAD;

            let blob_hash: [u8; 32] = {
                let mut h = Sha256::new();
                h.update(payload);
                h.finalize().into()
            };

            let header_bytes = blob_hash;
            let header_hash: [u8; 32] = {
                let mut h = Sha256::new();
                h.update(&header_bytes);
                h.finalize().into()
            };

            let ring = unsafe { &mut *caller.data().ring_va };
            let mut pos = 0usize;
            let buf = &mut ring.data;

            // 1. Blob record
            let ok1 = write_record(buf, &mut pos, 0, &blob_hash, payload);
            if !ok1 { return -1; }

            // KP1 : après push blob, avant push header
            if KILL_POINT == 1 { self_suspend_at(1); }

            // 2. Header record
            let ok2 = write_record(buf, &mut pos, 1, &header_hash, &header_bytes);
            if !ok2 { return -1; }

            // KP2 : après push header, avant push log_entry
            if KILL_POINT == 2 { self_suspend_at(2); }

            // 3. LogEntry record
            let ok3 = write_record(buf, &mut pos, 2, &header_hash, &[]);
            if !ok3 { return -1; }

            ring.data_len = pos as u32;
            core::sync::atomic::fence(Ordering::SeqCst);

            // KP3 : après write log_entry + fence + data_len, avant ep.call()
            if KILL_POINT == 3 { self_suspend_at(3); }

            // 4. IPC commit
            let ep = caller.data().ep;
            ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

            // KP4 : après retour de ep.call()
            if KILL_POINT == 4 { self_suspend_at(4); }

            unsafe {
                (*caller.data().ring_va).data_len = 0;
            }

            0
        })
        .expect("linker.func_wrap emit échoué");

    let mut store = Store::new(&engine, ctx);

    let instance = linker
        .instantiate(&mut store, &module)
        .expect("linker.instantiate échoué");

    sel4::debug_println!("[C7-crash] runtime: module WASM instancié");

    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .expect("get_typed_func 'run' échoué");

    run.call(&mut store, ()).expect("call run() échoué");

    sel4::debug_println!("[C7-crash] runtime: WASM run() terminé (KP0=nominal)");

    // Signal au superviseur (KP=0 : fin nominale)
    suspend_nfn.signal();
    sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
    unreachable!()
}
