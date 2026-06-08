//! Jalon C.11-prov — Root task superviseur (axe provenance P-δ)
//!
//! PHASE=0 (Phase A — P-δ + valid) :
//!   Spawn server.
//!   Sub-test P-δ (malformed) :
//!     Provision 32 octets de 0xDE dans la région module du runtime.
//!     Spawn runtime-malformed → Module::deserialize Err → ready_nfn → C11PROV_DELTA_PASS.
//!   Sub-test valid (happy path) :
//!     Provision PROV_CWASM (valide) dans la région module d'un nouveau runtime.
//!     Spawn runtime-valid → run() → commit → ready_nfn → oracle seq_a=1 → C11PROV_VALID_PASS.
//!   C11PROV_A_PASS si les deux PASS.
//!
//! PHASE=1 (Phase B — D-reopen P-γ) :
//!   Spawn server(phase=1) sur disk.img non wipé.
//!   VERIFY → verified==1 && seq_a==1 → C11PROV_GAMMA_PASS → C11PROV_PASS.
//!
//! C11PROV_PASS = P-δ + valid + P-γ (ADR-0048 §D2 sous-jalon C.11-prov).

#![no_std]
#![no_main]

extern crate alloc;

use core::ptr;

use object::{File, Object};
use sel4_root_task::{Never, root_task};
use sel4_common::child_vspace::{
    create_child_vspace, map_hardware_into_vspace, map_jit_frames_rw,
    provision_bytes_into_vspace,
};
use sel4_common::object_allocator::ObjectAllocator;

const SERVER_ELF_CONTENTS:  &[u8] = include_bytes!(env!("SERVER_ELF"));
const RUNTIME_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_ELF"));

/// cwasm valide compilé par build.rs depuis agent-prov.wat
static PROV_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-prov.cwasm"));

/// 32 octets de 0xDE : bytes malformés pour le test P-δ
static MALFORMED_BYTES: [u8; 32] = [0xDE; 32];

const fn parse_phase(s: Option<&'static str>) -> u64 {
    match s {
        None => 0,
        Some(v) => {
            let b = v.as_bytes();
            if b.len() == 1 && b[0] == b'1' { 1 } else { 0 }
        }
    }
}
const PHASE: u64 = parse_phase(option_env!("PHASE"));

const K: u64 = 1;

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

const AGENT_A_ID: sel4::Badge = 1;
const ORACLE_BADGE: sel4::Badge = 0xC1F1FE;
const VERIFY_BADGE: sel4::Badge = 0xC1F1FF;
const INIT_BADGE: sel4::Badge   = 0xC1F000;

const DMA_PAGES: usize = 16;
const VIRTIO_MMIO_BASE_PHYS: usize = 0x0a00_0000;
const VIRTIO_MMIO_PAGES: usize = 4;

// CNode runtime size_bits=8 (256 slots)
const CHILD_CNODE_SIZE_BITS_RUNTIME: usize = 8;
const RUNTIME_SLOT_EP:            u64 = 1;
const RUNTIME_SLOT_NFN:           u64 = 2; // ready_nfn
const RUNTIME_SLOT_TCB:           u64 = 3;
const RUNTIME_SLOT_VSPACE:        u64 = 4;
const RUNTIME_JIT_FRAME_BASE:     u64 = 5; // slots 5..132 = 128 frames JIT

// CNode serveur size_bits=2 (4 slots)
const CHILD_CNODE_SIZE_BITS_SERVER: usize = 2;
const SERVER_SLOT_EP:  u64 = 1;
const SERVER_SLOT_TCB: u64 = 2;

const JIT_POOL_PAGES: usize = 128;
const JIT_POOL_VA_BASE: usize = 0x4000_0000;

/// Base de la région module dans le VSpace runtime (lue par le runtime via get_module_bytes())
const MODULE_VA_BASE: usize = 0x5000_0000;

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

fn copy_frame(
    cnode: sel4::cap::CNode,
    src: sel4::cap::Granule,
    allocator: &mut ObjectAllocator,
) -> sel4::cap::Granule {
    let dest_slot = allocator.next_slot();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(dest_slot).cptr(),
        )
        .copy(&cnode.absolute_cptr(src.cptr()), sel4::CapRights::read_write())
        .unwrap();
    sel4::init_thread::Slot::from_index(dest_slot).cap()
}

fn spawn_server(
    allocator: &mut ObjectAllocator,
    server_image: &File<'_>,
    ring_frames: &[sel4::cap::Granule],
    dma_frames: &[sel4::cap::Granule],
    mmio_frames: &[sel4::cap::Granule],
    endpoint: sel4::cap::Endpoint,
    free_page_addr: usize,
) -> sel4::cap::Tcb {
    let cnode = sel4::init_thread::slot::CNODE.cap();

    let (vspace, ipc_buf_addr, ipc_buf_cap, _) = create_child_vspace(
        allocator,
        server_image,
        sel4::init_thread::slot::VSPACE.cap(),
        free_page_addr,
        sel4::init_thread::slot::ASID_POOL.cap(),
        ring_frames,
    );

    map_hardware_into_vspace(allocator, vspace, dma_frames, mmio_frames);

    let server_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS_SERVER);

    server_cnode
        .absolute_cptr_from_bits_with_depth(SERVER_SLOT_EP, CHILD_CNODE_SIZE_BITS_SERVER)
        .mint(&cnode.absolute_cptr(endpoint), sel4::CapRights::read_write(), 0)
        .unwrap();

    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();
    tcb.tcb_configure(
        sel4::init_thread::slot::NULL.cptr(),
        server_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS_SERVER),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    server_cnode
        .absolute_cptr_from_bits_with_depth(SERVER_SLOT_TCB, CHILD_CNODE_SIZE_BITS_SERVER)
        .mint(&cnode.absolute_cptr(tcb), sel4::CapRights::all(), 0)
        .unwrap();

    let mut ctx = sel4::UserContext::default();
    *ctx.pc_mut() = server_image.entry().try_into().unwrap();
    tcb.tcb_write_all_registers(true, &mut ctx).unwrap();
    tcb
}

/// Spawne un runtime qui lira ses octets module depuis MODULE_VA_BASE.
///
/// module_data : octets à provisionner dans la région module (malformés ou cwasm valide).
/// ready_nfn   : notification signalée par le runtime à la fin (Err ou Ok).
fn spawn_runtime_prov(
    allocator: &mut ObjectAllocator,
    runtime_image: &File<'_>,
    ring_frame: sel4::cap::Granule,
    endpoint: sel4::cap::Endpoint,
    ready_nfn: sel4::cap::Notification,
    module_data: &[u8],
    free_page_addr: usize,
) -> sel4::cap::Tcb {
    let cnode = sel4::init_thread::slot::CNODE.cap();

    let (vspace, ipc_buf_addr, ipc_buf_cap, _) = create_child_vspace(
        allocator,
        runtime_image,
        sel4::init_thread::slot::VSPACE.cap(),
        free_page_addr,
        sel4::init_thread::slot::ASID_POOL.cap(),
        &[ring_frame],
    );

    // Pool JIT W^X (pré-mappé RW+XN, remappé par le runtime via wasmtime_mprotect)
    let jit_frames = allocator.allocate_frames_batch(JIT_POOL_PAGES);
    map_jit_frames_rw(allocator, vspace, &jit_frames, JIT_POOL_VA_BASE);
    sel4::debug_println!(
        "[C11prov] {} frames JIT pré-mappées RW+XN à VA 0x{:08x}",
        JIT_POOL_PAGES, JIT_POOL_VA_BASE
    );

    // Région module (provisionnée par le superviseur)
    provision_bytes_into_vspace(
        allocator,
        vspace,
        sel4::init_thread::slot::VSPACE.cap(),
        free_page_addr,
        module_data,
        MODULE_VA_BASE,
    );
    sel4::debug_println!(
        "[C11prov] module provisionné ({} octets) à VA 0x{:08x}",
        module_data.len(), MODULE_VA_BASE
    );

    let runtime_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS_RUNTIME);

    // Slot 1 : EP commit
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_EP, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRightsBuilder::none().grant_reply(true).write(true).build(),
            AGENT_A_ID,
        )
        .unwrap();

    // Slot 2 : ready_nfn (write_only)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_NFN, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(&cnode.absolute_cptr(ready_nfn), sel4::CapRights::write_only(), 0)
        .unwrap();

    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();

    // TCB sans fault_ep (NULL → seL4 imprime sur UART si fault inattendu)
    tcb.tcb_configure(
        sel4::init_thread::slot::NULL.cptr(),
        runtime_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS_RUNTIME),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    // Slot 3 : TCB
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_TCB, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(&cnode.absolute_cptr(tcb), sel4::CapRights::all(), 0)
        .unwrap();

    // Slot 4 : VSpace
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_VSPACE, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .copy(&cnode.absolute_cptr(vspace), sel4::CapRights::all())
        .unwrap();

    // Slots 5..132 : caps frames JIT
    for (i, frame) in jit_frames.iter().enumerate() {
        let slot = RUNTIME_JIT_FRAME_BASE + i as u64;
        runtime_cnode
            .absolute_cptr_from_bits_with_depth(slot, CHILD_CNODE_SIZE_BITS_RUNTIME)
            .copy(&cnode.absolute_cptr(*frame), sel4::CapRights::read_write())
            .unwrap();
    }

    let mut ctx = sel4::UserContext::default();
    *ctx.pc_mut() = runtime_image.entry().try_into().unwrap();
    tcb.tcb_write_all_registers(true, &mut ctx).unwrap();

    tcb
}

#[root_task(heap_size = 512 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!(
        "=== OS-pour-IA : C.11-prov superviseur (PHASE={}) ===",
        PHASE
    );

    let mut allocator = ObjectAllocator::new(bootinfo);

    // DMA en PREMIER (L78)
    let (dma_frames, dma_paddr) = allocator.allocate_dma_frames_first(bootinfo, DMA_PAGES);
    sel4::debug_println!("[C11prov] DMA: {} pages à paddr=0x{:08x}", DMA_PAGES, dma_paddr);

    let mmio_frames =
        allocator.allocate_device_frames(bootinfo, VIRTIO_MMIO_BASE_PHYS, VIRTIO_MMIO_PAGES);

    let free_page_addr = init_free_page_addr(bootinfo);
    let cnode = sel4::init_thread::slot::CNODE.cap();

    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

    // Init cap
    let init_ep_slot = allocator.next_slot();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(init_ep_slot).cptr(),
        )
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRightsBuilder::none().grant_reply(true).write(true).build(),
            INIT_BADGE,
        )
        .unwrap();
    let init_ep: sel4::cap::Endpoint = sel4::init_thread::Slot::from_index(init_ep_slot).cap();

    // Oracle cap
    let oracle_ep_slot = allocator.next_slot();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(oracle_ep_slot).cptr(),
        )
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRightsBuilder::none().grant_reply(true).write(true).build(),
            ORACLE_BADGE,
        )
        .unwrap();
    let oracle_ep: sel4::cap::Endpoint = sel4::init_thread::Slot::from_index(oracle_ep_slot).cap();

    // Verify cap
    let verify_ep_slot = allocator.next_slot();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(verify_ep_slot).cptr(),
        )
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRightsBuilder::none().grant_reply(true).write(true).build(),
            VERIFY_BADGE,
        )
        .unwrap();
    let verify_ep: sel4::cap::Endpoint = sel4::init_thread::Slot::from_index(verify_ep_slot).cap();

    let server_image  = File::parse(SERVER_ELF_CONTENTS).unwrap();
    let runtime_image = File::parse(RUNTIME_ELF_CONTENTS).unwrap();

    if PHASE == 0 {
        // ── Phase A : P-δ + valid ─────────────────────────────────────────────

        let ring_orig   = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
        let ring_server = copy_frame(cnode, ring_orig, &mut allocator);

        let _server_tcb = spawn_server(
            &mut allocator, &server_image, &[ring_server],
            &dma_frames, &mmio_frames, endpoint, free_page_addr,
        );
        sel4::debug_println!("[C11prov] Phase A: server spawné");

        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 0; // phase A
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C11prov] Phase A: server init OK");

        // ── Sub-test P-δ (malformed) ──────────────────────────────────────────
        sel4::debug_println!("[C11prov] Phase A: sub-test P-delta (bytes malformés)");

        let nfn_malformed = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();
        let ring_malformed = copy_frame(cnode, ring_orig, &mut allocator);

        let _rt_malformed = spawn_runtime_prov(
            &mut allocator, &runtime_image,
            ring_malformed, endpoint, nfn_malformed,
            &MALFORMED_BYTES,
            free_page_addr,
        );
        sel4::debug_println!("[C11prov] Phase A: runtime malformé spawné");

        // Attendre ready_nfn (runtime retourne Err sur Module::deserialize)
        nfn_malformed.wait();
        sel4::debug_println!("[C11prov] Phase A: ready_nfn malformed reçu");

        // Oracle → seq_a doit être 0 (aucun commit)
        let _oracle_reply = oracle_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let seq_a = sel4::with_ipc_buffer(|buf| buf.msg_regs()[0]);
        sel4::debug_println!("[C11prov] P-delta: oracle → seq_a={} (attendu=0)", seq_a);

        let delta_pass = seq_a == 0;
        if delta_pass {
            sel4::debug_println!("C11PROV_DELTA_PASS");
        } else {
            sel4::debug_println!("C11PROV_DELTA_FAIL: seq_a={} attendu=0", seq_a);
        }

        // ── Sub-test valid (happy path) ───────────────────────────────────────
        sel4::debug_println!("[C11prov] Phase A: sub-test valid (cwasm depuis canal non-trusted)");

        let nfn_valid = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();
        let ring_valid = copy_frame(cnode, ring_orig, &mut allocator);

        let _rt_valid = spawn_runtime_prov(
            &mut allocator, &runtime_image,
            ring_valid, endpoint, nfn_valid,
            PROV_CWASM,
            free_page_addr,
        );
        sel4::debug_println!("[C11prov] Phase A: runtime valide spawné ({} octets cwasm)", PROV_CWASM.len());

        // Attendre ready_nfn (run() terminé, commit effectué)
        nfn_valid.wait();
        sel4::debug_println!("[C11prov] Phase A: ready_nfn valid reçu");

        // Oracle → seq_a doit être 1
        let _oracle_reply = oracle_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let seq_a = sel4::with_ipc_buffer(|buf| buf.msg_regs()[0]);
        sel4::debug_println!("[C11prov] valid: oracle → seq_a={} (attendu=1)", seq_a);

        let valid_pass = seq_a == 1;
        if valid_pass {
            sel4::debug_println!("C11PROV_VALID_PASS");
        } else {
            sel4::debug_println!("C11PROV_VALID_FAIL: seq_a={} attendu=1", seq_a);
        }

        // Verdict Phase A
        if delta_pass && valid_pass {
            sel4::debug_println!("C11PROV_A_PASS");
        } else {
            sel4::debug_println!("C11PROV_A_FAIL: delta={} valid={}", delta_pass, valid_pass);
        }

    } else {
        // ── Phase B : D-reopen ────────────────────────────────────────────────
        let _server_tcb = spawn_server(
            &mut allocator, &server_image, &[],
            &dma_frames, &mmio_frames, endpoint, free_page_addr,
        );

        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 1; // phase B
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C11prov] Phase B: server init OK (reopen)");

        let _verify_reply = verify_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let (verified, seq_a) = sel4::with_ipc_buffer(|buf| {
            (buf.msg_regs()[0], buf.msg_regs()[1])
        });
        sel4::debug_println!(
            "[C11prov] Phase B: verified={} seq_a={} K={}",
            verified, seq_a, K
        );

        if verified == K && seq_a == K {
            sel4::debug_println!("C11PROV_GAMMA_PASS");
            sel4::debug_println!("C11PROV_PASS");
        } else {
            sel4::debug_println!(
                "C11PROV_GAMMA_FAIL: verified={} seq_a={} K={}",
                verified, seq_a, K
            );
        }
    }

    sel4::init_thread::suspend_self()
}
