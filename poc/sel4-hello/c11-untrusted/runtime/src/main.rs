//! Jalon C.11 — Runtime process WASM non confié
//!
//! MODULE_KIND=1 → module OOB (P-α) : commit puis trap OOB
//! MODULE_KIND=2 → module boucle infinie (P-β) : commit puis boucle infinie
//!
//! Cap layout CNode runtime (size_bits=8) :
//!   Slot 1 : EP commit (badge=AGENT_A_ID, GrantReply+Write)
//!   Slot 2 : nfn (write_only) — signalé par "started" (loop) ou unused (oob)
//!   Slot 3 : TCB
//!   Slot 4 : VSpace (pour wasmtime_mprotect)
//!   Slots 5..132 : caps frames JIT
//!   Slot 133 : fault_ep — UNIQUEMENT pour MODULE_KIND=1 (OOB)

#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::Ordering;

use sel4::CapTypeForFrameObjectOfFixedSize;
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Linker, Module, Store};

mod platform;
mod runtime;

const EP_SLOT:  u64 = 1;
const NFN_SLOT: u64 = 2;
const TCB_SLOT: u64 = 3;

// Sélection du module WASM selon MODULE_KIND (env var au moment du build)
const fn parse_module_kind(s: Option<&'static str>) -> u64 {
    match s {
        None => 1,
        Some(v) => {
            let b = v.as_bytes();
            if b.len() == 1 && b[0] == b'2' { 2 } else { 1 }
        }
    }
}
const MODULE_KIND: u64 = parse_module_kind(option_env!("MODULE_KIND"));

// Modules WASM pré-compilés par build.rs
static AGENT_OOB_CWASM:  &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-oob.cwasm"));
static AGENT_LOOP_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-loop.cwasm"));

static EMIT_PAYLOAD: &[u8] = b"C11_UNTRUSTED_COMMIT";

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
    if *pos + needed > buf.len() { return false; }
    buf[*pos] = kind; *pos += 1;
    buf[*pos..*pos + 32].copy_from_slice(hash); *pos += 32;
    let plen = payload.len() as u32;
    buf[*pos..*pos + 4].copy_from_slice(&plen.to_le_bytes()); *pos += 4;
    buf[*pos..*pos + payload.len()].copy_from_slice(payload); *pos += payload.len();
    true
}

struct EmitCtx {
    ep: sel4::cap::Endpoint,
    ring_va: *mut RingBuffer,
}

unsafe impl Send for EmitCtx {}
unsafe impl Sync for EmitCtx {}

pub fn main() -> ! {
    sel4::debug_println!("[C11] runtime: démarrage MODULE_KIND={}", MODULE_KIND);

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let ring_va = get_ring_va();

    unsafe { (*ring_va).data_len = 0; }

    let ctx = EmitCtx { ep, ring_va };

    let engine = Engine::default();

    // Sélectionner le module selon MODULE_KIND
    let cwasm: &[u8] = if MODULE_KIND == 2 {
        AGENT_LOOP_CWASM
    } else {
        AGENT_OOB_CWASM
    };

    let module = unsafe {
        Module::deserialize(&engine, cwasm).expect("Module::deserialize échoué")
    };

    let mut linker: Linker<EmitCtx> = Linker::new(&engine);

    // Host function "emit" : commit Q3-C dans le ring, call ep
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

            let ok1 = write_record(buf, &mut pos, 0, &blob_hash, payload);
            if !ok1 { return -1; }
            let ok2 = write_record(buf, &mut pos, 1, &header_hash, &header_bytes);
            if !ok2 { return -1; }
            let ok3 = write_record(buf, &mut pos, 2, &header_hash, &[]);
            if !ok3 { return -1; }

            ring.data_len = pos as u32;
            core::sync::atomic::fence(Ordering::SeqCst);

            // Commit via IPC (séquentiel + bloquant)
            let ep = caller.data().ep;
            ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

            unsafe { (*caller.data().ring_va).data_len = 0; }
            0
        })
        .expect("linker.func_wrap emit échoué");

    // Host function "started" : signale la notification NFN_SLOT
    // Pour le module OOB, cette fonction n'est pas importée → pas de problème.
    // Pour le module loop, elle est appelée après emit().
    linker
        .func_wrap("env", "started", |_caller: wasmtime::Caller<'_, EmitCtx>| {
            let nfn = sel4::cap::Notification::from_bits(NFN_SLOT);
            nfn.signal();
        })
        .expect("linker.func_wrap started échoué");

    let mut store = Store::new(&engine, ctx);

    let instance = linker
        .instantiate(&mut store, &module)
        .expect("linker.instantiate échoué");

    sel4::debug_println!("[C11] runtime: module WASM instancié (MODULE_KIND={})", MODULE_KIND);

    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .expect("get_typed_func 'run' échoué");

    // Pour MODULE_KIND=1 (OOB) : run.call() déclenche un trap OOB
    //   → wasmtime_longjmp → panic → abort!() dans runtime.rs → CPU fault → fault_ep seL4
    // Pour MODULE_KIND=2 (loop) : run.call() ne retourne jamais
    //   → le superviseur suspend le TCB de l'extérieur
    run.call(&mut store, ()).expect("call run() échoué");

    // MODULE_KIND=2 ne devrait pas arriver ici (boucle infinie)
    // MODULE_KIND=1 ne devrait pas arriver ici (trap avant)
    sel4::debug_println!("[C11] runtime: run() terminé (inattendu)");

    sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
    unreachable!()
}
