//! Jalon C.10 — Runtime process W^X
//!
//! Charge le module WASM agent.cwasm (pré-compilé AOT).
//! Fournit la host function emit() (protocole Q3-C, K=1 commit).
//! Wasmtime JIT utilise le pool de frames dédiées avec W^X actif (platform.rs).
//!
//! Séquence :
//!   1. Wasmtime JIT compile + exécute agent.cwasm → K=1 commit via emit()
//!   2. Signal done_nfn → superviseur reçoit "C10_HAPPY_PASS"
//!   3. Test négatif W^X : écriture sur la première page RX → VM fault seL4
//!      → superviseur reçoit le fault via fault_ep → "C10_NEG_PASS"
//!
//! Cap layout CNode runtime (size_bits=8) :
//!   Slot 1 : EP commit (badge=AGENT_A_ID)
//!   Slot 2 : done_nfn (write_only)
//!   Slot 3 : TCB
//!   Slot 4 : VSpace (pour wasmtime_mprotect)
//!   Slots 5..132 : frames JIT

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

// Module WASM pré-compilé par build.rs
static AGENT_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent.cwasm"));

static EMIT_PAYLOAD: &[u8] = b"C10_WX_COMMIT_FROM_JIT_AGENT";

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
    sel4::debug_println!("[C10] runtime W^X: démarrage");

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let ring_va = get_ring_va();

    unsafe { (*ring_va).data_len = 0; }

    let ctx = EmitCtx { ep, ring_va };

    // JIT compile + exécute le module avec W^X actif (platform.rs gère le remap)
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

            let ok1 = write_record(buf, &mut pos, 0, &blob_hash, payload);
            if !ok1 { return -1; }
            let ok2 = write_record(buf, &mut pos, 1, &header_hash, &header_bytes);
            if !ok2 { return -1; }
            let ok3 = write_record(buf, &mut pos, 2, &header_hash, &[]);
            if !ok3 { return -1; }

            ring.data_len = pos as u32;
            core::sync::atomic::fence(Ordering::SeqCst);

            // Commit via IPC
            let ep = caller.data().ep;
            ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

            unsafe { (*caller.data().ring_va).data_len = 0; }
            0
        })
        .expect("linker.func_wrap emit échoué");

    let mut store = Store::new(&engine, ctx);

    let instance = linker
        .instantiate(&mut store, &module)
        .expect("linker.instantiate échoué");

    sel4::debug_println!("[C10] runtime W^X: module WASM instancié (JIT W^X actif)");

    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .expect("get_typed_func 'run' échoué");

    run.call(&mut store, ()).expect("call run() échoué");

    sel4::debug_println!("[C10] runtime W^X: run() terminé — K=1 commit sous W^X ✓");

    // ── Signaler done_nfn : happy path W^X confirmé ───────────────────────────
    // Le superviseur attend ce signal pour imprimer C10_HAPPY_PASS.
    let done_nfn = sel4::cap::Notification::from_bits(NFN_SLOT);
    done_nfn.signal();

    // ── Test négatif W^X : écriture sur une page RX → VM fault seL4 ──────────
    // Le superviseur observe le VM fault via fault_ep → C10_NEG_PASS.
    // Cette écriture NE doit PAS réussir (invariant W^X).
    let rx_page_va = unsafe { platform::FIRST_RX_PAGE_VA };

    sel4::debug_println!(
        "[C10] test négatif : tentative écriture sur page RX va=0x{:08x}",
        rx_page_va
    );

    unsafe {
        // Écriture sur page en état RX → VM fault seL4 (ADR-0047 §D3 critère 2)
        // Si cette instruction s'exécute sans fault → W^X est CASSÉ.
        (rx_page_va as *mut u8).write_volatile(0xCC);

        // Si on arrive ici, le test négatif a ÉCHOUÉ (page encore writable)
        sel4::debug_println!("[C10] C10_NEG_FAIL: écriture réussie sur page RX — W^X non actif !");
        done_nfn.signal(); // signal supplémentaire pour ne pas bloquer le superviseur
    }

    sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
    unreachable!()
}
