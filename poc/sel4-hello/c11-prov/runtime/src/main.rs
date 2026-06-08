//! Jalon C.11-prov — Runtime : charge le module WASM depuis canal non-trusted.
//!
//! Le module est provisionné par le superviseur dans la région MODULE_VA_BASE.
//! Format : [len: u64 LE][cwasm bytes...].
//!
//! Si Module::deserialize Err  → signal ready_nfn + suspend (P-δ PASS pour bytes malformés).
//! Si Module::deserialize Ok   → run() → commit → signal ready_nfn + suspend.
//!
//! Cap layout CNode runtime (size_bits=8) :
//!   Slot 1 : EP commit (badge=AGENT_A_ID, GrantReply+Write)
//!   Slot 2 : ready_nfn (write_only) — signalé sur Err ET succès
//!   Slot 3 : TCB
//!   Slot 4 : VSpace (pour wasmtime_mprotect)
//!   Slots 5..132 : caps frames JIT

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

/// Base de la région module provisionnée par le superviseur.
/// Format : [len: u64 LE][cwasm bytes...]
const MODULE_VA_BASE: usize = 0x5000_0000;

fn get_module_bytes() -> &'static [u8] {
    unsafe {
        let base = MODULE_VA_BASE as *const u8;
        let len_bytes = *(base as *const [u8; 8]);
        let module_len = u64::from_le_bytes(len_bytes) as usize;
        core::slice::from_raw_parts(base.add(8), module_len)
    }
}

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

static EMIT_PAYLOAD: &[u8] = b"C11PROV_COMMIT";

pub fn main() -> ! {
    sel4::debug_println!("[C11prov] runtime: démarrage");

    let ep  = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let nfn = sel4::cap::Notification::from_bits(NFN_SLOT);
    let tcb = sel4::cap::Tcb::from_bits(TCB_SLOT);
    let ring_va = get_ring_va();

    unsafe { (*ring_va).data_len = 0; }

    let module_bytes = get_module_bytes();
    sel4::debug_println!("[C11prov] runtime: module_bytes.len()={}", module_bytes.len());

    let engine = Engine::default();

    match unsafe { Module::deserialize(&engine, module_bytes) } {
        Err(_) => {
            sel4::debug_println!("[C11prov] runtime: Module::deserialize Err — bytes malformés");
            sel4::debug_println!("C11PROV_DESERIALIZE_ERR");
            nfn.signal();
            tcb.tcb_suspend().unwrap();
            unreachable!()
        }
        Ok(module) => {
            sel4::debug_println!("[C11prov] runtime: Module::deserialize Ok — instanciation...");

            let ctx = EmitCtx { ep, ring_va };
            let mut linker: Linker<EmitCtx> = Linker::new(&engine);

            linker.func_wrap("env", "emit", |caller: wasmtime::Caller<'_, EmitCtx>| -> i32 {
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

                let ep = caller.data().ep;
                ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

                unsafe { (*caller.data().ring_va).data_len = 0; }
                0
            }).expect("func_wrap emit failed");

            let mut store = Store::new(&engine, ctx);
            let instance = linker
                .instantiate(&mut store, &module)
                .expect("instantiate failed");

            sel4::debug_println!("[C11prov] runtime: module instancié — appel run()...");

            let run = instance
                .get_typed_func::<(), ()>(&mut store, "run")
                .expect("get_typed_func run failed");

            run.call(&mut store, ()).expect("run() failed");

            sel4::debug_println!("[C11prov] runtime: run() terminé OK");
        }
    }

    sel4::debug_println!("C11PROV_RUNTIME_DONE");
    nfn.signal();
    sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
    unreachable!()
}
