//! Jalon C.8 — Root task superviseur
//!
//! Topologie identique à C.7-crash + backend redb sur virtio-blk :
//!   - Server (VSpace B) : reçoit commits via ring SPSC, store dans redb/virtio-blk,
//!                         oracle badge=0xC8FE, init badge=0xC8_0000
//!   - Runtime A (VSpace A1) : badge=AGENT_A_ID, KILL_POINT configuré (KP1-4)
//!   - Runtime B (VSpace A2) : badge=AGENT_B_ID, KILL_POINT=0 (nominal)
//!
//! Séquence de démarrage :
//!   1. Allouer frames DMA (paddr) + frames MMIO device
//!   2. Créer VSpace serveur avec DMA+MMIO+rings mappés
//!   3. Envoyer init IPC (badge=INIT_BADGE, dma_paddr) au serveur
//!   4. Attendre acquittement serveur ("ready")
//!   5. Spawner runtimes A+B
//!   6. Attendre suspensions + oracle query → assertions I3-N + I4
//!
//! Critère de sortie : KP{N}_PASS (4 runs) → C8_PASS

#![no_std]
#![no_main]

extern crate alloc;

use core::ptr;

use object::{File, Object};
use sel4_root_task::{Never, root_task};

mod child_vspace;
mod object_allocator;

use child_vspace::{
    DMA_VA_BASE, SCAN_VA, create_child_vspace, map_hardware_into_vspace,
};
use object_allocator::ObjectAllocator;

// ELFs embarqués — chemins fournis par build.rs
const SERVER_ELF_CONTENTS: &[u8] = include_bytes!(env!("SERVER_ELF"));
const RUNTIME_A_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_A_ELF"));
const RUNTIME_B_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_B_ELF"));

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

// Badges agents (ADR-0044 §D2)
const AGENT_A_ID: sel4::Badge = 1;
const AGENT_B_ID: sel4::Badge = 2;
const ORACLE_BADGE: sel4::Badge = 0xC8FE;

// Badge init : superviseur → serveur pour init hardware (avant runtimes)
const INIT_BADGE: sel4::Badge = 0xC8_0000;

// DMA layout (identique à C.4/C.5)
const DMA_PAGES: usize = 16;
const DMA_SIZE: usize = DMA_PAGES * GRANULE_SIZE;

// MMIO layout (identique à C.4/C.5)
const VIRTIO_MMIO_BASE_PHYS: usize = 0x0a00_0000;
const VIRTIO_MMIO_PAGES: usize = 4; // 16KB couvrant 32 slots × 0x200

// CNode enfant : size_bits=2 → 4 slots (0=NULL, 1=EP, 2=NFN, 3=TCB)
const CHILD_CNODE_SIZE_BITS: usize = 2;
const CHILD_SLOT_EP: u64 = 1;
const CHILD_SLOT_NFN: u64 = 2;
const CHILD_SLOT_TCB: u64 = 3;

// Kill point du superviseur (pour assertions I3-N)
const fn parse_kill_point(s: Option<&'static str>) -> u32 {
    match s {
        None => 1,
        Some(v) => {
            let b = v.as_bytes();
            if b.len() == 1 && b[0] == b'1' { 1 }
            else if b.len() == 1 && b[0] == b'2' { 2 }
            else if b.len() == 1 && b[0] == b'3' { 3 }
            else if b.len() == 1 && b[0] == b'4' { 4 }
            else { 1 }
        }
    }
}
const KILL_POINT: u32 = parse_kill_point(option_env!("KILL_POINT"));

// Free page pour la copie des segments ELF
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
        .copy(
            &cnode.absolute_cptr(src.cptr()),
            sel4::CapRights::read_write(),
        )
        .unwrap();
    sel4::init_thread::Slot::from_index(dest_slot).cap()
}

fn spawn_runtime(
    allocator: &mut ObjectAllocator,
    runtime_image: &File<'_>,
    ring_frame: sel4::cap::Granule,
    endpoint: sel4::cap::Endpoint,
    suspend_nfn: sel4::cap::Notification,
    agent_id: sel4::Badge,
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

    let child_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS);

    // Slot 1 : commit-cap badgée (I-cap ADR-0044 §D2, GrantReply requis L70)
    child_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_EP, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::all(),
            agent_id,
        )
        .unwrap();

    // Slot 2 : suspend notification (write_only)
    child_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_NFN, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(suspend_nfn),
            sel4::CapRights::write_only(),
            0,
        )
        .unwrap();

    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();
    tcb.tcb_configure(
        sel4::init_thread::slot::NULL.cptr(),
        child_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    // Slot 3 : own TCB cap
    child_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_TCB, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(tcb),
            sel4::CapRights::all(),
            0,
        )
        .unwrap();

    let mut ctx = sel4::UserContext::default();
    *ctx.pc_mut() = runtime_image.entry().try_into().unwrap();
    tcb.tcb_write_all_registers(true, &mut ctx).unwrap();
    tcb
}

#[root_task(heap_size = 128 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.8 ===");
    sel4::debug_println!("    KILL_POINT={} (ADR-0045)", KILL_POINT);

    // ── 0. Initialisation ────────────────────────────────────────────────────

    let mut allocator = ObjectAllocator::new(bootinfo);

    // DMA frames en PREMIER (avant tout autre alloc → paddr = ut_paddr)
    // ADR-0045 garde-fou : durabilité niveau 1 — paddr passé au serveur via init IPC
    let (dma_frames, dma_paddr) =
        allocator.allocate_dma_frames_first(bootinfo, DMA_PAGES);
    sel4::debug_println!("[C8] DMA: {} pages à paddr=0x{:08x}", DMA_PAGES, dma_paddr);

    // Device frames pour le scan MMIO virtio (4 pages à 0x0a000000)
    let mmio_frames =
        allocator.allocate_device_frames(bootinfo, VIRTIO_MMIO_BASE_PHYS, VIRTIO_MMIO_PAGES);
    sel4::debug_println!("[C8] MMIO device frames: {} pages à 0x{:08x}", VIRTIO_MMIO_PAGES, VIRTIO_MMIO_BASE_PHYS);

    let free_page_addr = init_free_page_addr(bootinfo);
    let cnode = sel4::init_thread::slot::CNODE.cap();

    // ── 1. Allouer les objets partagés ───────────────────────────────────────

    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();
    let suspend_nfn_a = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();
    let suspend_nfn_b = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();

    // Ring A + copie server
    let ring_a_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
    let ring_a_server = copy_frame(cnode, ring_a_orig, &mut allocator);

    // Ring B + copie server
    let ring_b_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
    let ring_b_server = copy_frame(cnode, ring_b_orig, &mut allocator);

    // Init cap : superviseur → serveur (badge=INIT_BADGE, GrantReply requis)
    let init_ep_slot = allocator.next_slot();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(init_ep_slot).cptr(),
        )
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::all(),
            INIT_BADGE,
        )
        .unwrap();
    let init_ep: sel4::cap::Endpoint =
        sel4::init_thread::Slot::from_index(init_ep_slot).cap();

    // Oracle cap : superviseur → serveur (badge=ORACLE_BADGE)
    let oracle_ep_slot = allocator.next_slot();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(oracle_ep_slot).cptr(),
        )
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::all(),
            ORACLE_BADGE,
        )
        .unwrap();
    let oracle_ep: sel4::cap::Endpoint =
        sel4::init_thread::Slot::from_index(oracle_ep_slot).cap();

    // ── 2. Créer le VSpace du server (rings + DMA + MMIO) ────────────────────

    let server_image = File::parse(SERVER_ELF_CONTENTS).unwrap();
    let (server_vspace, server_ipc_buf_addr, server_ipc_buf_cap, _) =
        create_child_vspace(
            &mut allocator,
            &server_image,
            sel4::init_thread::slot::VSPACE.cap(),
            free_page_addr,
            sel4::init_thread::slot::ASID_POOL.cap(),
            &[ring_a_server, ring_b_server],
        );

    // Mapper DMA + MMIO dans le VSpace du server
    map_hardware_into_vspace(&mut allocator, server_vspace, &dma_frames, &mmio_frames);
    sel4::debug_println!(
        "[C8] server VSpace: DMA@0x{:08x}, MMIO@0x{:08x}",
        DMA_VA_BASE, SCAN_VA
    );

    let server_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS);

    // Slot 1 : endpoint (read_write pour recv)
    server_cnode
        .absolute_cptr_from_bits_with_depth(1, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::read_write(),
            0,
        )
        .unwrap();

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

    // Slot 2 : own TCB cap
    server_cnode
        .absolute_cptr_from_bits_with_depth(2, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(server_tcb),
            sel4::CapRights::all(),
            0,
        )
        .unwrap();

    let mut server_ctx = sel4::UserContext::default();
    *server_ctx.pc_mut() = server_image.entry().try_into().unwrap();
    server_tcb
        .tcb_write_all_registers(true, &mut server_ctx)
        .unwrap();
    sel4::debug_println!("[C8] server démarré");

    // ── 3. Init IPC : passer dma_paddr au server → attendre "ready" ──────────
    //
    // Le serveur initialise virtio-blk + redb avant d'entrer dans sa boucle.
    // Durabilité niveau 1 (ADR-0045 §Q2=α) — ack ne garantit pas le flush media.

    sel4::with_ipc_buffer_mut(|buf| {
        buf.msg_regs_mut()[0] = dma_paddr as u64;
    });
    let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 1));
    sel4::debug_println!("[C8] server init (virtio-blk + redb) OK");

    // ── 4. Spawner runtime A (instrumenté KP={}) et runtime B (nominal) ──────

    let runtime_a_image = File::parse(RUNTIME_A_ELF_CONTENTS).unwrap();
    let _runtime_a_tcb = spawn_runtime(
        &mut allocator,
        &runtime_a_image,
        ring_a_orig,
        endpoint,
        suspend_nfn_a,
        AGENT_A_ID,
        free_page_addr,
    );
    sel4::debug_println!(
        "[C8] runtime A (badge={}, KP={}) démarré",
        AGENT_A_ID, KILL_POINT
    );

    let runtime_b_image = File::parse(RUNTIME_B_ELF_CONTENTS).unwrap();
    let _runtime_b_tcb = spawn_runtime(
        &mut allocator,
        &runtime_b_image,
        ring_b_orig,
        endpoint,
        suspend_nfn_b,
        AGENT_B_ID,
        free_page_addr,
    );
    sel4::debug_println!(
        "[C8] runtime B (badge={}, nominal) démarré",
        AGENT_B_ID
    );

    // ── 5. Attendre les signaux suspend des 2 runtimes ───────────────────────

    suspend_nfn_a.wait();
    sel4::debug_println!("[C8] superviseur : suspend_A reçu (KP={})", KILL_POINT);

    suspend_nfn_b.wait();
    sel4::debug_println!("[C8] superviseur : suspend_B reçu");

    // ── 6. Oracle query → asserter I3-N + I4 ────────────────────────────────

    let _oracle_reply = oracle_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

    let (seq_a, seq_b) = sel4::with_ipc_buffer_mut(|buf| {
        (buf.msg_regs()[0], buf.msg_regs()[1])
    });

    sel4::debug_println!(
        "[C8] oracle: seq_a={}, seq_b={} (KP={})",
        seq_a, seq_b, KILL_POINT
    );

    // I3-N : KP1/2/3 → seq_a=0 ; KP4 → seq_a=1
    let expected_seq_a: u64 = if KILL_POINT == 4 { 1 } else { 0 };
    // I4 : seq_b=1 (non-interférence d'intégrité, ADR-0044)
    let expected_seq_b: u64 = 1;

    let i3n_ok = seq_a == expected_seq_a;
    let i4_ok = seq_b == expected_seq_b;

    sel4::debug_println!(
        "[C8] I3-N: seq_a={} expected={} → {}",
        seq_a, expected_seq_a,
        if i3n_ok { "OK" } else { "FAIL" }
    );
    sel4::debug_println!(
        "[C8] I4: seq_b={} expected={} → {}",
        seq_b, expected_seq_b,
        if i4_ok { "OK" } else { "FAIL" }
    );

    if i3n_ok && i4_ok {
        sel4::debug_println!("KP{}_PASS", KILL_POINT);
    } else {
        sel4::debug_println!("KP{}_FAIL: i3n={} i4={}", KILL_POINT, i3n_ok, i4_ok);
    }

    sel4::init_thread::suspend_self()
}
