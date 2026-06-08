//! Jalon C.9 — Root task superviseur (Phase A ou Phase B)
//!
//! PHASE=0 (Phase A — écriture) :
//!   Spawn server (phase=0) + runtime.
//!   Attend signal "done" du runtime (K=100 commits).
//!   Output "REOPEN_A_PASS".
//!
//! PHASE=1 (Phase B — vérification persistance) :
//!   Spawn server (phase=1) sur le même disk.img (non wipé).
//!   Envoie VERIFY_BADGE → serveur lit redb, vérifie K=100 entrées.
//!   Output "C9_PASS" si verified==K && seq_a==K, sinon "C9_FAIL".
//!
//! Critère D-reopen (ADR-0046) : Phase B valide que les données écrites en Phase A
//! ont survécu au redémarrage QEMU (disk.img partagé, sans `dd` entre les deux runs).

#![no_std]
#![no_main]

extern crate alloc;

use core::ptr;

use object::{File, Object};
use sel4_root_task::{Never, root_task};

// Migration vers sel4-common (ADR-0047 — crate commune née en C.10)
use sel4_common::child_vspace::{
    DMA_VA_BASE, SCAN_VA, create_child_vspace, map_hardware_into_vspace,
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

const K: u64 = 100;

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

const AGENT_A_ID: sel4::Badge = 1;
const INIT_BADGE: sel4::Badge = 0xC9_0000;
const VERIFY_BADGE: sel4::Badge = 0xC9FF;

const DMA_PAGES: usize = 16;
const VIRTIO_MMIO_BASE_PHYS: usize = 0x0a00_0000;
const VIRTIO_MMIO_PAGES: usize = 4;

const CHILD_CNODE_SIZE_BITS: usize = 2;
const CHILD_SLOT_EP: u64 = 1;
const CHILD_SLOT_TCB: u64 = 2;
const RUNTIME_SLOT_EP: u64 = 1;
const RUNTIME_SLOT_NFN: u64 = 2;
const RUNTIME_SLOT_TCB: u64 = 3;

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
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS);

    // Slot 1 : endpoint (read_write pour recv)
    server_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_EP, CHILD_CNODE_SIZE_BITS)
        .mint(&cnode.absolute_cptr(endpoint), sel4::CapRights::read_write(), 0)
        .unwrap();

    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();
    tcb.tcb_configure(
        sel4::init_thread::slot::NULL.cptr(),
        server_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    // Slot 2 : own TCB cap.
    // all() conservé : l'invocation TCB (tcb_suspend) n'est pas rights-gated comme un
    // endpoint ; le bit Grant n'y a pas de sens et la réduction serait sans effet
    // fonctionnel — non réduite faute de bénéfice testable (S2).
    server_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_TCB, CHILD_CNODE_SIZE_BITS)
        .mint(&cnode.absolute_cptr(tcb), sel4::CapRights::all(), 0)
        .unwrap();

    let mut ctx = sel4::UserContext::default();
    *ctx.pc_mut() = server_image.entry().try_into().unwrap();
    tcb.tcb_write_all_registers(true, &mut ctx).unwrap();
    tcb
}

fn spawn_runtime(
    allocator: &mut ObjectAllocator,
    runtime_image: &File<'_>,
    ring_frame: sel4::cap::Granule,
    endpoint: sel4::cap::Endpoint,
    done_nfn: sel4::cap::Notification,
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

    let runtime_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS);

    // Slot 1 : commit-cap badgée AGENT_A_ID.
    // Least-privilege (S2) : seL4_Call exige Write (send) + GrantReply (reply cap),
    // PAS Grant (transfert de caps via IPC) ni Read — cf. L70. all() était sur-privilégié.
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_EP, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRightsBuilder::none().grant_reply(true).write(true).build(),
            AGENT_A_ID,
        )
        .unwrap();

    // Slot 2 : done notification (write_only)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_NFN, CHILD_CNODE_SIZE_BITS)
        .mint(&cnode.absolute_cptr(done_nfn), sel4::CapRights::write_only(), 0)
        .unwrap();

    let tcb = allocator.allocate_fixed_sized::<sel4::cap_type::Tcb>();
    tcb.tcb_configure(
        sel4::init_thread::slot::NULL.cptr(),
        runtime_cnode,
        sel4::CNodeCapData::new(0, sel4::WORD_SIZE - CHILD_CNODE_SIZE_BITS),
        vspace,
        ipc_buf_addr as sel4::Word,
        ipc_buf_cap,
    )
    .unwrap();

    // Slot 3 : own TCB cap (all() conservé — cf. note Slot 2 server, S2)
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(RUNTIME_SLOT_TCB, CHILD_CNODE_SIZE_BITS)
        .mint(&cnode.absolute_cptr(tcb), sel4::CapRights::all(), 0)
        .unwrap();

    let mut ctx = sel4::UserContext::default();
    *ctx.pc_mut() = runtime_image.entry().try_into().unwrap();
    tcb.tcb_write_all_registers(true, &mut ctx).unwrap();
    tcb
}

#[root_task(heap_size = 128 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : C.9 superviseur (PHASE={}) ===", PHASE);

    let mut allocator = ObjectAllocator::new(bootinfo);

    // DMA en PREMIER (L78 : watermark=0 → paddr=ut_paddr)
    let (dma_frames, dma_paddr) = allocator.allocate_dma_frames_first(bootinfo, DMA_PAGES);
    sel4::debug_println!("[C9] DMA: {} pages à paddr=0x{:08x}", DMA_PAGES, dma_paddr);

    let mmio_frames =
        allocator.allocate_device_frames(bootinfo, VIRTIO_MMIO_BASE_PHYS, VIRTIO_MMIO_PAGES);
    sel4::debug_println!("[C9] MMIO: {} pages à 0x{:08x}", VIRTIO_MMIO_PAGES, VIRTIO_MMIO_BASE_PHYS);

    let free_page_addr = init_free_page_addr(bootinfo);
    let cnode = sel4::init_thread::slot::CNODE.cap();

    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

    // Init cap (badge=INIT_BADGE)
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

    // Verify cap (badge=VERIFY_BADGE) — utilisée en Phase B
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
        // ── Phase A : spawn server (phase=0) + runtime ───────────────────────

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
        sel4::debug_println!("[C9] Phase A: server spawné");

        // Init IPC → server (phase=0)
        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 0; // phase A
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C9] Phase A: server init OK");

        let runtime_image = File::parse(RUNTIME_ELF_CONTENTS).unwrap();
        let _runtime_tcb = spawn_runtime(
            &mut allocator,
            &runtime_image,
            ring_a_orig,
            endpoint,
            done_nfn,
            free_page_addr,
        );
        sel4::debug_println!("[C9] Phase A: runtime spawné (K={} commits)", K);

        // Attendre signal du runtime
        done_nfn.wait();
        sel4::debug_println!("[C9] Phase A: {} commits flush sur redb/virtio-blk", K);
        sel4::debug_println!("REOPEN_A_PASS");

    } else {
        // ── Phase B : spawn server (phase=1), vérifier K entrées ─────────────

        let _server_tcb = spawn_server(
            &mut allocator,
            &server_image,
            &[], // pas de ring en Phase B
            &dma_frames,
            &mmio_frames,
            endpoint,
            free_page_addr,
        );
        sel4::debug_println!("[C9] Phase B: server spawné");

        // Init IPC → server (phase=1)
        sel4::with_ipc_buffer_mut(|buf| {
            buf.msg_regs_mut()[0] = dma_paddr as u64;
            buf.msg_regs_mut()[1] = 1; // phase B
        });
        let _init_reply = init_ep.call(sel4::MessageInfo::new(0, 0, 0, 2));
        sel4::debug_println!("[C9] Phase B: server init OK (redb rouvert sur disk.img existant)");

        // Envoyer VERIFY_BADGE → server lit et vérifie K entrées dans redb
        let _verify_reply = verify_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));
        let (verified, seq_a) = sel4::with_ipc_buffer(|buf| {
            (buf.msg_regs()[0], buf.msg_regs()[1])
        });

        sel4::debug_println!(
            "[C9] Phase B: verified={} seq_a={} K={}",
            verified, seq_a, K
        );

        if verified == K && seq_a == K {
            sel4::debug_println!("C9_PASS");
        } else {
            sel4::debug_println!("C9_FAIL: verified={} seq_a={} expected K={}", verified, seq_a, K);
        }
    }

    sel4::init_thread::suspend_self()
}
