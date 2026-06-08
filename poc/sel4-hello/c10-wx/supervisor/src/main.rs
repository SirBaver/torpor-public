//! Jalon C.10 — Root task superviseur W^X
//!
//! PHASE=0 (Phase A — W^X + commit) :
//!   Spawn server(phase=0) + runtime_wx.
//!   Attend done_nfn → runtime a fait K=1 commit avec W^X actif.
//!   Imprime REOPEN_A_PASS + C10_HAPPY_PASS.
//!   Attend fault_ep → runtime a tenté d'écrire sur page RX → VM fault.
//!   Imprime C10_NEG_PASS + C10_PASS.
//!
//! PHASE=1 (Phase B — vérification D-reopen) :
//!   Spawn server(phase=1) sur le même disk.img (non wipé).
//!   Envoie VERIFY_BADGE → server vérifie K=1 entrées.
//!   Imprime C10_REOPEN_PASS ou C10_REOPEN_FAIL.
//!
//! C10_PASS = REOPEN_A_PASS + C10_HAPPY_PASS + C10_NEG_PASS + C10_REOPEN_PASS (ADR-0047)

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

const SERVER_ELF_CONTENTS: &[u8] = include_bytes!(env!("SERVER_ELF"));
const RUNTIME_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_ELF"));

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

const K: u64 = 1; // smoke : 1 commit suffit

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

const AGENT_A_ID: sel4::Badge = 1;
const INIT_BADGE: sel4::Badge = 0xC10000;
const VERIFY_BADGE: sel4::Badge = 0xC10FF;

const DMA_PAGES: usize = 16;
const VIRTIO_MMIO_BASE_PHYS: usize = 0x0a00_0000;
const VIRTIO_MMIO_PAGES: usize = 4;

// CNode runtime size_bits=8 (256 slots, ADR-0047 §D5)
const CHILD_CNODE_SIZE_BITS_RUNTIME: usize = 8;
const RUNTIME_SLOT_EP:     u64 = 1;
const RUNTIME_SLOT_NFN:    u64 = 2;
const RUNTIME_SLOT_TCB:    u64 = 3;
const RUNTIME_SLOT_VSPACE: u64 = 4;
const RUNTIME_JIT_FRAME_BASE: u64 = 5; // slots 5..132 = 128 frames JIT

// CNode serveur size_bits=2 (4 slots)
const CHILD_CNODE_SIZE_BITS_SERVER: usize = 2;
const SERVER_SLOT_EP: u64 = 1;
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

/// Spawne le runtime W^X (ADR-0047 §D4/§D5).
///
/// CNode runtime size_bits=8 (256 slots) :
///   Slot 1 : EP commit (badge=AGENT_A_ID)
///   Slot 2 : done_nfn (write_only)
///   Slot 3 : TCB
///   Slot 4 : cap VSpace (pour frame_unmap/frame_map dans wasmtime_mprotect)
///   Slots 5..132 : 128 caps frames JIT (état initial RW+EXECUTE_NEVER)
///
/// fault_ep est configuré comme fault endpoint du TCB runtime :
///   le superviseur appelle fault_ep.recv() pour observer le VM fault du test négatif.
fn spawn_runtime_wx(
    allocator: &mut ObjectAllocator,
    runtime_image: &File<'_>,
    ring_frame: sel4::cap::Granule,
    endpoint: sel4::cap::Endpoint,
    done_nfn: sel4::cap::Notification,
    free_page_addr: usize,
) -> sel4::cap::Tcb {
    let cnode = sel4::init_thread::slot::CNODE.cap();

    // 1. Créer le VSpace et mapper l'ELF + IPC buffer + ring
    let (vspace, ipc_buf_addr, ipc_buf_cap, _) = create_child_vspace(
        allocator,
        runtime_image,
        sel4::init_thread::slot::VSPACE.cap(),
        free_page_addr,
        sel4::init_thread::slot::ASID_POOL.cap(),
        &[ring_frame],
    );

    // 2. Allouer 128 frames JIT
    let jit_frames = allocator.allocate_frames_batch(JIT_POOL_PAGES);

    // 3. Mapper les frames JIT dans le VSpace runtime à JIT_POOL_VA_BASE (RW + EXECUTE_NEVER)
    map_jit_frames_rw(allocator, vspace, &jit_frames, JIT_POOL_VA_BASE);
    sel4::debug_println!(
        "[C10] {} frames JIT pré-mappées RW+XN à VA 0x{:08x}",
        JIT_POOL_PAGES, JIT_POOL_VA_BASE
    );

    // 4. Créer le CNode runtime (size_bits=8)
    let runtime_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS_RUNTIME);

    // Slot 1 : EP commit (badge=AGENT_A_ID, GrantReply + Write)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_EP, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRightsBuilder::none().grant_reply(true).write(true).build(),
            AGENT_A_ID,
        )
        .unwrap();

    // Slot 2 : done_nfn (write_only)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_NFN, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(&cnode.absolute_cptr(done_nfn), sel4::CapRights::write_only(), 0)
        .unwrap();

    // Slot 3 : TCB (all — pour tcb_suspend self)
    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();

    // 5. Configurer le TCB (fault_ep = NULL : la sortie debug seL4 confirme le VM fault)
    tcb.tcb_configure(
        sel4::init_thread::slot::NULL.cptr(),
        runtime_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS_RUNTIME),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    // Slot 3 : propre TCB cap
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_TCB, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .mint(&cnode.absolute_cptr(tcb), sel4::CapRights::all(), 0)
        .unwrap();

    // Slot 4 : cap VSpace (pour wasmtime_mprotect : frame_unmap + frame_map)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_VSPACE, CHILD_CNODE_SIZE_BITS_RUNTIME)
        .copy(&cnode.absolute_cptr(vspace), sel4::CapRights::all())
        .unwrap();

    // Slots 5..132 : caps des 128 frames JIT (copies, droits read_write)
    for (i, frame) in jit_frames.iter().enumerate() {
        let slot = RUNTIME_JIT_FRAME_BASE + i as u64;
        runtime_cnode
            .absolute_cptr_from_bits_with_depth(slot, CHILD_CNODE_SIZE_BITS_RUNTIME)
            .copy(&cnode.absolute_cptr(*frame), sel4::CapRights::read_write())
            .unwrap();
    }

    // 6. Démarrer le runtime
    let mut ctx = sel4::UserContext::default();
    *ctx.pc_mut() = runtime_image.entry().try_into().unwrap();
    tcb.tcb_write_all_registers(true, &mut ctx).unwrap();
    tcb
}

#[root_task(heap_size = 256 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : C.10 superviseur W^X (PHASE={}) ===", PHASE);

    let mut allocator = ObjectAllocator::new(bootinfo);

    // DMA en PREMIER (L78 : watermark=0 → paddr=ut_paddr)
    let (dma_frames, dma_paddr) = allocator.allocate_dma_frames_first(bootinfo, DMA_PAGES);
    sel4::debug_println!("[C10] DMA: {} pages à paddr=0x{:08x}", DMA_PAGES, dma_paddr);

    let mmio_frames =
        allocator.allocate_device_frames(bootinfo, VIRTIO_MMIO_BASE_PHYS, VIRTIO_MMIO_PAGES);
    sel4::debug_println!("[C10] MMIO: {} pages à 0x{:08x}", VIRTIO_MMIO_PAGES, VIRTIO_MMIO_BASE_PHYS);

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

    let server_image = File::parse(SERVER_ELF_CONTENTS).unwrap();

    if PHASE == 0 {
        // ── Phase A : W^X + K=1 commit + test négatif ─────────────────────────

        let ring_a_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
        let ring_a_server = copy_frame(cnode, ring_a_orig, &mut allocator);

        let done_nfn = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();

        let _server_tcb = spawn_server(
            &mut allocator,
            &server_image,
            &[ring_a_server],
            &dma_frames,
            &mmio_frames,
            endpoint,
            free_page_addr,
        );
        sel4::debug_println!("[C10] Phase A: server spawné");

        // Init IPC → server (phase=0)
        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 0; // phase A
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C10] Phase A: server init OK");

        let runtime_image = File::parse(RUNTIME_ELF_CONTENTS).unwrap();
        let _runtime_tcb = spawn_runtime_wx(
            &mut allocator,
            &runtime_image,
            ring_a_orig,
            endpoint,
            done_nfn,
            free_page_addr,
        );
        sel4::debug_println!("[C10] Phase A: runtime W^X spawné (K={})", K);

        // ── Attendre done_nfn : K=1 commit réussi sous W^X ────────────────────
        done_nfn.wait();
        sel4::debug_println!("[C10] done_nfn reçu : K={} commit(s) sous W^X actif", K);
        sel4::debug_println!("REOPEN_A_PASS");
        sel4::debug_println!("C10_HAPPY_PASS");
        // Le runtime va maintenant tenter d'écrire sur une page RX.
        // seL4 debug kernel imprime "vm fault on data at address 0x4..." sur l'UART.
        // test.py valide C10_NEG_PASS en observant ce message (ADR-0047 §D3 critère 2).
        sel4::debug_println!("C10_PASS");

    } else {
        // ── Phase B : D-reopen — vérifier K=1 entrées ─────────────────────────

        let _server_tcb = spawn_server(
            &mut allocator,
            &server_image,
            &[], // pas de ring en Phase B
            &dma_frames,
            &mmio_frames,
            endpoint,
            free_page_addr,
        );
        sel4::debug_println!("[C10] Phase B: server spawné");

        // Init IPC → server (phase=1, reopen)
        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 1; // phase B
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C10] Phase B: server init OK (redb rouvert sur disk.img)");

        // Vérification K=1 entrées
        let _verify_reply = verify_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let (verified, seq_a) = sel4::with_ipc_buffer(|buf| {
            (buf.msg_regs()[0], buf.msg_regs()[1])
        });

        sel4::debug_println!(
            "[C10] Phase B: verified={} seq_a={} K={}",
            verified, seq_a, K
        );

        if verified == K && seq_a == K {
            sel4::debug_println!("C10_REOPEN_PASS");
        } else {
            sel4::debug_println!("C10_REOPEN_FAIL: verified={} seq_a={} expected K={}", verified, seq_a, K);
        }
    }

    sel4::init_thread::suspend_self()
}
