//! Jalon C.11 — Root task superviseur WASM non confié
//!
//! PHASE=0 (Phase A — P-α + P-β) :
//!   Spawn server(phase=0) + runtime-oob + runtime-loop (séquentiels).
//!
//!   P-α (OOB) :
//!     runtime-oob exécute agent-oob.wat : commit → trap OOB → panic → abort → CPU fault.
//!     fault_ep reçoit le fault → seq_a == 1 → C11_ALPHA_PASS.
//!
//!   P-β (loop) :
//!     runtime-loop exécute agent-loop.wat : commit → started() → boucle infinie.
//!     loop_nfn reçu → tcb_suspend externe → seq_a == 2 → C11_BETA_PASS.
//!
//!   C11_AB_PASS si les deux PASS.
//!
//! PHASE=1 (Phase B — D-reopen P-γ) :
//!   Spawn server(phase=1) sur disk.img non wipé.
//!   VERIFY_BADGE → verified==2 && seq_a==2 → C11_GAMMA_PASS → C11_PASS.
//!
//! C11_PASS = P-α PASS + P-β PASS + P-γ PASS (ADR-0048 §D2).

#![no_std]
#![no_main]

extern crate alloc;

use core::ptr;

use object::{File, Object};
use sel4_root_task::{Never, root_task};
use sel4_common::child_vspace::{
    create_child_vspace, map_hardware_into_vspace, map_jit_frames_rw,
};
use sel4_common::object_allocator::ObjectAllocator;

// ELFs embarqués dans le superviseur
const SERVER_ELF_CONTENTS:       &[u8] = include_bytes!(env!("SERVER_ELF"));
const RUNTIME_OOB_ELF_CONTENTS:  &[u8] = include_bytes!(env!("RUNTIME_OOB_ELF"));
const RUNTIME_LOOP_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_LOOP_ELF"));

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

const K: u64 = 2; // OOB commit + LOOP commit

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

const AGENT_A_ID: sel4::Badge = 1;
const ORACLE_BADGE: sel4::Badge = 0xC11FE;
const VERIFY_BADGE: sel4::Badge = 0xC11FF;
const INIT_BADGE: sel4::Badge   = 0xC11000;

const DMA_PAGES: usize = 16;
const VIRTIO_MMIO_BASE_PHYS: usize = 0x0a00_0000;
const VIRTIO_MMIO_PAGES: usize = 4;

// CNode runtime size_bits=8 (256 slots)
const CHILD_CNODE_SIZE_BITS_RUNTIME: usize = 8;
const RUNTIME_SLOT_EP:       u64 = 1;
const RUNTIME_SLOT_NFN:      u64 = 2;
const RUNTIME_SLOT_TCB:      u64 = 3;
const RUNTIME_SLOT_VSPACE:   u64 = 4;
const RUNTIME_JIT_FRAME_BASE: u64 = 5; // slots 5..132 = 128 frames JIT
const RUNTIME_SLOT_FAULT_EP: u64 = 133; // slot 133 = fault_ep (OOB uniquement)

// CNode serveur size_bits=2 (4 slots)
const CHILD_CNODE_SIZE_BITS_SERVER: usize = 2;
const SERVER_SLOT_EP:  u64 = 1;
const SERVER_SLOT_TCB: u64 = 2;

const JIT_POOL_PAGES: usize = 128;
const JIT_POOL_VA_BASE: usize = 0x4000_0000;

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

/// Spawne le runtime OOB (P-α).
///
/// CNode size_bits=8 (256 slots) :
///   Slot 1  : EP commit (badge=AGENT_A_ID, GrantReply+Write)
///   Slot 2  : NULL (started n'est pas appelé par le module OOB)
///   Slot 3  : TCB
///   Slot 4  : VSpace
///   Slots 5..132 : 128 frames JIT
///   Slot 133 : fault_ep (CapRights::all() — seL4 doit pouvoir send sur cet EP)
///
/// Le TCB est configuré avec fault_ep comme fault endpoint.
fn spawn_runtime_oob(
    allocator: &mut ObjectAllocator,
    runtime_image: &File<'_>,
    ring_frame: sel4::cap::Granule,
    endpoint: sel4::cap::Endpoint,
    fault_ep: sel4::cap::Endpoint,
    free_page_addr: usize,
) -> (sel4::cap::Tcb, sel4::cap::CNode) {
    let cnode = sel4::init_thread::slot::CNODE.cap();

    let (vspace, ipc_buf_addr, ipc_buf_cap, _) = create_child_vspace(
        allocator,
        runtime_image,
        sel4::init_thread::slot::VSPACE.cap(),
        free_page_addr,
        sel4::init_thread::slot::ASID_POOL.cap(),
        &[ring_frame],
    );

    let jit_frames = allocator.allocate_frames_batch(JIT_POOL_PAGES);
    map_jit_frames_rw(allocator, vspace, &jit_frames, JIT_POOL_VA_BASE);
    sel4::debug_println!(
        "[C11] {} frames JIT OOB pré-mappées RW+XN à VA 0x{:08x}",
        JIT_POOL_PAGES, JIT_POOL_VA_BASE
    );

    let runtime_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS_RUNTIME);

    // Slot 1 : EP commit (badge=AGENT_A_ID, GrantReply+Write)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_EP, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRightsBuilder::none().grant_reply(true).write(true).build(),
            AGENT_A_ID,
        )
        .unwrap();

    // Slot 2 : NULL (started non importé par module OOB)
    // (pas de mint nécessaire — slot vide = NULL par défaut)

    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();

    // Slot 133 : fault_ep (CapRights::all() — seL4 fault delivery)
    // Doit être mintée AVANT tcb_configure pour que le CPtr soit résolu dans le CSpace du runtime.
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_FAULT_EP, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(
            &cnode.absolute_cptr(fault_ep),
            sel4::CapRights::all(),
            0,
        )
        .unwrap();

    // Configurer TCB avec fault_ep = CPtr::from_bits(133) résolu dans le CSpace du runtime
    tcb.tcb_configure(
        sel4::CPtr::from_bits(RUNTIME_SLOT_FAULT_EP),
        runtime_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS_RUNTIME),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    // Slot 3 : TCB cap propre
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_TCB, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(&cnode.absolute_cptr(tcb), sel4::CapRights::all(), 0)
        .unwrap();

    // Slot 4 : VSpace
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_VSPACE, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .copy(&cnode.absolute_cptr(vspace), sel4::CapRights::all())
        .unwrap();

    // Slots 5..132 : caps des 128 frames JIT
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

    (tcb, runtime_cnode)
}

/// Spawne le runtime LOOP (P-β).
///
/// CNode size_bits=8 (256 slots) :
///   Slot 1  : EP commit (badge=AGENT_A_ID, GrantReply+Write)
///   Slot 2  : loop_nfn (write_only) — signalé quand le module appelle "started"
///   Slot 3  : TCB
///   Slot 4  : VSpace
///   Slots 5..132 : 128 frames JIT
///   Pas de fault_ep (fault_ep = NULL → seL4 imprime sur UART, processus kilé)
fn spawn_runtime_loop(
    allocator: &mut ObjectAllocator,
    runtime_image: &File<'_>,
    ring_frame: sel4::cap::Granule,
    endpoint: sel4::cap::Endpoint,
    loop_nfn: sel4::cap::Notification,
    free_page_addr: usize,
) -> (sel4::cap::Tcb, sel4::cap::CNode) {
    let cnode = sel4::init_thread::slot::CNODE.cap();

    let (vspace, ipc_buf_addr, ipc_buf_cap, _) = create_child_vspace(
        allocator,
        runtime_image,
        sel4::init_thread::slot::VSPACE.cap(),
        free_page_addr,
        sel4::init_thread::slot::ASID_POOL.cap(),
        &[ring_frame],
    );

    let jit_frames = allocator.allocate_frames_batch(JIT_POOL_PAGES);
    map_jit_frames_rw(allocator, vspace, &jit_frames, JIT_POOL_VA_BASE);
    sel4::debug_println!(
        "[C11] {} frames JIT LOOP pré-mappées RW+XN à VA 0x{:08x}",
        JIT_POOL_PAGES, JIT_POOL_VA_BASE
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

    // Slot 2 : loop_nfn (write_only) — signalé par "started"
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_NFN, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(&cnode.absolute_cptr(loop_nfn), sel4::CapRights::write_only(), 0)
        .unwrap();

    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();

    // Configurer TCB sans fault_ep (NULL → seL4 default)
    tcb.tcb_configure(
        sel4::init_thread::slot::NULL.cptr(),
        runtime_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS_RUNTIME),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    // Slot 3 : TCB cap propre
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_TCB, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(&cnode.absolute_cptr(tcb), sel4::CapRights::all(), 0)
        .unwrap();

    // Slot 4 : VSpace
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_VSPACE, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .copy(&cnode.absolute_cptr(vspace), sel4::CapRights::all())
        .unwrap();

    // Slots 5..132 : caps des 128 frames JIT
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

    (tcb, runtime_cnode)
}

#[root_task(heap_size = 512 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : C.11 superviseur WASM non confié (PHASE={}) ===", PHASE);

    let mut allocator = ObjectAllocator::new(bootinfo);

    // DMA en PREMIER (L78 : watermark=0 → paddr=ut_paddr)
    let (dma_frames, dma_paddr) = allocator.allocate_dma_frames_first(bootinfo, DMA_PAGES);
    sel4::debug_println!("[C11] DMA: {} pages à paddr=0x{:08x}", DMA_PAGES, dma_paddr);

    let mmio_frames =
        allocator.allocate_device_frames(bootinfo, VIRTIO_MMIO_BASE_PHYS, VIRTIO_MMIO_PAGES);
    sel4::debug_println!("[C11] MMIO: {} pages à 0x{:08x}", VIRTIO_MMIO_PAGES, VIRTIO_MMIO_BASE_PHYS);

    let free_page_addr = init_free_page_addr(bootinfo);
    let cnode = sel4::init_thread::slot::CNODE.cap();

    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

    // Init cap (badge=INIT_BADGE) : superviseur → serveur
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

    // Verify cap (badge=VERIFY_BADGE) : superviseur → serveur Phase B
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

    // Oracle cap (badge=ORACLE_BADGE) : superviseur → serveur pour query seq
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

    let server_image = File::parse(SERVER_ELF_CONTENTS).unwrap();

    if PHASE == 0 {
        // ── Phase A : P-α + P-β ────────────────────────────────────────────────

        // Un seul ring frame partagé entre les deux runtimes (séquentiels)
        let ring_orig   = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
        let ring_server = copy_frame(cnode, ring_orig, &mut allocator);

        // Allouer fault_ep AVANT de spawner les runtimes
        let fault_ep = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

        let _server_tcb = spawn_server(
            &mut allocator,
            &server_image,
            &[ring_server],
            &dma_frames,
            &mmio_frames,
            endpoint,
            free_page_addr,
        );
        sel4::debug_println!("[C11] Phase A: server spawné");

        // Init IPC → server (phase=0)
        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 0; // phase A
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C11] Phase A: server init OK");

        // ── Sous-test P-α (OOB) ───────────────────────────────────────────────
        sel4::debug_println!("[C11] Phase A: lancement sous-test P-alpha (OOB)");

        let oob_image = File::parse(RUNTIME_OOB_ELF_CONTENTS).unwrap();
        let (_oob_tcb, _oob_cnode) = spawn_runtime_oob(
            &mut allocator,
            &oob_image,
            ring_orig,
            endpoint,
            fault_ep,
            free_page_addr,
        );
        sel4::debug_println!("[C11] Phase A: runtime OOB spawné");

        // Attendre le fault (OOB trap → panic → abort → CPU fault → fault_ep)
        sel4::debug_println!("[C11] Phase A: attente fault_ep (trap OOB attendu)");
        fault_ep.recv(());
        sel4::debug_println!("[C11] Phase A: fault_ep reçu — trap OOB confirmé");

        // Query oracle → seq_a doit être 1 (le module OOB a fait 1 commit avant de crasher)
        sel4::with_ipc_buffer_mut(|buf| {
            let _ = buf; // ipc_buffer_mut pas nécessaire pour call sans args
        });
        let _oracle_reply = oracle_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let seq_a = sel4::with_ipc_buffer(|buf| buf.msg_regs()[0]);
        sel4::debug_println!("[C11] Phase A P-alpha: oracle → seq_a={}", seq_a);

        let alpha_pass = seq_a == 1;
        if alpha_pass {
            sel4::debug_println!("C11_ALPHA_PASS");
        } else {
            sel4::debug_println!("C11_ALPHA_FAIL: seq_a={} attendu=1", seq_a);
        }

        // ── Sous-test P-β (LOOP) ──────────────────────────────────────────────
        sel4::debug_println!("[C11] Phase A: lancement sous-test P-beta (LOOP)");

        let loop_nfn = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();
        let loop_image = File::parse(RUNTIME_LOOP_ELF_CONTENTS).unwrap();

        // Pour que le serveur puisse lire les données commitées par le runtime LOOP,
        // il faut que le runtime LOOP écrive dans le MÊME ring physique que le serveur.
        // ring_orig est déjà mappé dans le VSpace OOB ; on crée une nouvelle copie de cap
        // pointant sur le même frame physique pour le mapper dans le VSpace LOOP.
        // Le serveur lit depuis ring_server (copie de ring_orig) → cohérence assurée.
        let ring_loop = copy_frame(cnode, ring_orig, &mut allocator);

        let (loop_tcb, _loop_cnode) = spawn_runtime_loop(
            &mut allocator,
            &loop_image,
            ring_loop,
            endpoint,
            loop_nfn,
            free_page_addr,
        );
        sel4::debug_println!("[C11] Phase A: runtime LOOP spawné");

        // Attendre loop_nfn : le module a commit et est entré dans la boucle infinie
        sel4::debug_println!("[C11] Phase A: attente loop_nfn (commit + boucle attendus)");
        loop_nfn.wait();
        sel4::debug_println!("[C11] Phase A: loop_nfn reçu — runtime LOOP a commité et entre en boucle");

        // Préemption externe watchdog : suspendre le TCB LOOP
        loop_tcb.tcb_suspend().unwrap();
        sel4::debug_println!("[C11] Phase A: runtime LOOP suspendu (watchdog externe)");

        // Query oracle → seq_a doit être 2 (OOB + LOOP = 2 commits)
        let _oracle_reply = oracle_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let seq_a = sel4::with_ipc_buffer(|buf| buf.msg_regs()[0]);
        sel4::debug_println!("[C11] Phase A P-beta: oracle → seq_a={}", seq_a);

        let beta_pass = seq_a == 2;
        if beta_pass {
            sel4::debug_println!("C11_BETA_PASS");
        } else {
            sel4::debug_println!("C11_BETA_FAIL: seq_a={} attendu=2", seq_a);
        }

        // Verdict Phase A
        if alpha_pass && beta_pass {
            sel4::debug_println!("C11_AB_PASS");
        } else {
            sel4::debug_println!("C11_AB_FAIL: alpha={} beta={}", alpha_pass, beta_pass);
        }

    } else {
        // ── Phase B : D-reopen — vérifier K=2 entrées ─────────────────────────

        let _server_tcb = spawn_server(
            &mut allocator,
            &server_image,
            &[], // pas de ring en Phase B
            &dma_frames,
            &mmio_frames,
            endpoint,
            free_page_addr,
        );
        sel4::debug_println!("[C11] Phase B: server spawné");

        // Init IPC → server (phase=1, reopen)
        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 1; // phase B
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C11] Phase B: server init OK (redb rouvert sur disk.img)");

        // Vérification K=2 entrées
        let _verify_reply = verify_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let (verified, seq_a) = sel4::with_ipc_buffer(|buf| {
            (buf.msg_regs()[0], buf.msg_regs()[1])
        });

        sel4::debug_println!(
            "[C11] Phase B: verified={} seq_a={} K={}",
            verified, seq_a, K
        );

        if verified == K && seq_a == K {
            sel4::debug_println!("C11_GAMMA_PASS");
            sel4::debug_println!("C11_PASS");
        } else {
            sel4::debug_println!("C11_GAMMA_FAIL: verified={} seq_a={} expected K={}", verified, seq_a, K);
        }
    }

    sel4::init_thread::suspend_self()
}
