//! Jalon C.6 — Root task superviseur
//!
//! Topologie 2-processus :
//!   - Server (VSpace B) : recv endpoint → lit ring → commit Q3-C en RAM → reply
//!   - Runtime (VSpace A) : WASM emit() → écrit ring → ep.call() → C6_PASS → signal done
//!
//! Critère de succès : UART QEMU affiche C6_PASS.

#![no_std]
#![no_main]

extern crate alloc;

use core::ptr;

use object::{File, Object};
use sel4_root_task::{Never, root_task};

mod child_vspace;
mod object_allocator;

use child_vspace::create_child_vspace;
use object_allocator::ObjectAllocator;

// ELFs embarqués — chemins fournis par build.rs via vars d'env SERVER_ELF / RUNTIME_ELF
const SERVER_ELF_CONTENTS: &[u8] = include_bytes!(env!("SERVER_ELF"));
const RUNTIME_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_ELF"));

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

// child_cnode_size_bits = 2 → 4 slots par CNode enfant
const CHILD_CNODE_SIZE_BITS: usize = 2;

// Free page pour la copie des segments ELF dans create_child_vspace
#[repr(C, align(4096))]
struct FreePagePlaceholder(#[allow(dead_code)] [u8; GRANULE_SIZE]);
static mut FREE_PAGE_PLACEHOLDER: FreePagePlaceholder = FreePagePlaceholder([0; GRANULE_SIZE]);

fn init_free_page_addr(bootinfo: &sel4::BootInfoPtr) -> usize {
    let addr = ptr::addr_of!(FREE_PAGE_PLACEHOLDER) as usize;
    get_user_image_frame_slot(bootinfo, addr)
        .cap()
        .frame_unmap()
        .unwrap();
    addr
}

fn get_user_image_frame_slot(
    bootinfo: &sel4::BootInfoPtr,
    addr: usize,
) -> sel4::init_thread::Slot<sel4::cap_type::Granule> {
    unsafe extern "C" {
        static __executable_start: usize;
    }
    let user_image_addr = ptr::addr_of!(__executable_start) as usize;
    bootinfo
        .user_image_frames()
        .index(addr / GRANULE_SIZE - user_image_addr / GRANULE_SIZE)
}

#[root_task(heap_size = 128 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.6 ===");
    sel4::debug_println!("    Topologie 2-processus : runtime + server (ADR-0043)");

    let mut allocator = ObjectAllocator::new(bootinfo);
    let free_page_addr = init_free_page_addr(bootinfo);

    // ── 1. Allouer les objets partagés ────────────────────────────────────────

    // Endpoint IPC entre runtime (caller) et server (receiver)
    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

    // Notification done : runtime → superviseur
    let done_nfn = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();

    // Ring buffer physique : 1 granule, mappé 2 fois (server + runtime)
    let ring_frame_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();

    // Copie du ring frame cap pour le 2e mapping
    let ring_copy_slot = allocator.next_slot();
    let cnode = sel4::init_thread::slot::CNODE.cap();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(ring_copy_slot)
                .cptr(),
        )
        .copy(
            &cnode.absolute_cptr(ring_frame_orig.cptr()),
            sel4::CapRights::read_write(),
        )
        .unwrap();
    let ring_frame_copy: sel4::cap::Granule =
        sel4::init_thread::Slot::from_index(ring_copy_slot).cap();

    // ── 2. Créer le VSpace du server ──────────────────────────────────────────

    let server_image = File::parse(SERVER_ELF_CONTENTS).unwrap();

    let (server_vspace, server_ipc_buf_addr, server_ipc_buf_cap, _server_ring_va) =
        create_child_vspace(
            &mut allocator,
            &server_image,
            sel4::init_thread::slot::VSPACE.cap(),
            free_page_addr,
            sel4::init_thread::slot::ASID_POOL.cap(),
            ring_frame_orig,
        );

    // CNode server : 4 slots
    let server_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS);

    // Slot 1: endpoint (read_write pour recv + reply)
    server_cnode
        .absolute_cptr_from_bits_with_depth(1, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::read_write(),
            0,
        )
        .unwrap();

    // TCB server
    let server_tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();
    server_tcb
        .tcb_configure(
            sel4::init_thread::slot::NULL.cptr(),
            server_cnode,
            sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS),
            server_vspace,
            server_ipc_buf_addr as sel4::Word,
            server_ipc_buf_cap,
        )
        .unwrap();

    // Slot 2: own TCB cap du server
    server_cnode
        .absolute_cptr_from_bits_with_depth(2, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(server_tcb),
            sel4::CapRights::all(),
            0,
        )
        .unwrap();

    // Démarrer le server
    let mut server_ctx = sel4::UserContext::default();
    *server_ctx.pc_mut() = server_image.entry().try_into().unwrap();
    server_tcb
        .tcb_write_all_registers(true, &mut server_ctx)
        .unwrap();

    sel4::debug_println!("[C6] server démarré");

    // ── 3. Créer le VSpace du runtime ─────────────────────────────────────────

    let runtime_image = File::parse(RUNTIME_ELF_CONTENTS).unwrap();

    let (runtime_vspace, runtime_ipc_buf_addr, runtime_ipc_buf_cap, _runtime_ring_va) =
        create_child_vspace(
            &mut allocator,
            &runtime_image,
            sel4::init_thread::slot::VSPACE.cap(),
            free_page_addr,
            sel4::init_thread::slot::ASID_POOL.cap(),
            ring_frame_copy,
        );

    // CNode runtime : 4 slots
    let runtime_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS);

    // Slot 1: endpoint (all rights pour call — seL4_Call nécessite Write + GrantReply)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(1, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::all(),
            0,
        )
        .unwrap();

    // Slot 2: done notification (write_only)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(2, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(done_nfn),
            sel4::CapRights::write_only(),
            0,
        )
        .unwrap();

    // TCB runtime
    let runtime_tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();
    runtime_tcb
        .tcb_configure(
            sel4::init_thread::slot::NULL.cptr(),
            runtime_cnode,
            sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS),
            runtime_vspace,
            runtime_ipc_buf_addr as sel4::Word,
            runtime_ipc_buf_cap,
        )
        .unwrap();

    // Slot 3: own TCB cap du runtime
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(3, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(runtime_tcb),
            sel4::CapRights::all(),
            0,
        )
        .unwrap();

    // Démarrer le runtime
    let mut runtime_ctx = sel4::UserContext::default();
    *runtime_ctx.pc_mut() = runtime_image.entry().try_into().unwrap();
    runtime_tcb
        .tcb_write_all_registers(true, &mut runtime_ctx)
        .unwrap();

    sel4::debug_println!("[C6] runtime démarré");

    // ── 4. Attendre le signal done du runtime ────────────────────────────────

    done_nfn.wait();

    sel4::debug_println!("[C6] superviseur : signal done reçu");

    // Suspendre le thread init
    sel4::init_thread::suspend_self()
}
