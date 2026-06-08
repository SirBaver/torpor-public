//! Jalon C.7-A — Root task superviseur
//!
//! Topologie N=2 agents (ADR-0044) :
//!   - Server (VSpace B) : reçoit commits de 2 agents via endpoint badgé, indexe par (agent_id, k)
//!   - Runtime A (VSpace A1) : badge=AGENT_A_ID, ring A → commit → signal done_a
//!   - Runtime B (VSpace A2) : badge=AGENT_B_ID, ring B → commit → signal done_b
//!
//! Invariant I-cap (ADR-0044) : chaque runtime reçoit une commit-cap badgée
//! avec son propre agent_id, jamais la cap non-badgée ni celle de l'autre agent.
//!
//! Critère de succès : UART QEMU affiche C7-A_PASS.

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

// Badges agents (ADR-0044 §D2) : badge = agent_id pur, kind dans le label
const AGENT_A_ID: sel4::Badge = 1;
const AGENT_B_ID: sel4::Badge = 2;

// child_cnode_size_bits = 2 → 4 slots par CNode enfant (slots 0-3)
const CHILD_CNODE_SIZE_BITS: usize = 2;

// Slots dans les CNodes enfants
// Slot 0: NULL
// Slot 1: commit-cap (endpoint, badge=agent_id, CapRights::all() — GrantReply requis L70)
// Slot 2: done notification (write_only)
// Slot 3: own TCB cap (pour tcb_suspend par le runtime lui-même)
const CHILD_SLOT_EP: u64 = 1;
const CHILD_SLOT_NFN: u64 = 2;
const CHILD_SLOT_TCB: u64 = 3;

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
    done_nfn: sel4::cap::Notification,
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

    // Slot 1: commit-cap badgée agent_id (I-cap ADR-0044 §D2)
    // CapRights::all() pour Write + GrantReply (seL4_Call requis L70)
    child_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_EP as u64, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::all(),
            agent_id,
        )
        .unwrap();

    // Slot 2: done notification (write_only)
    child_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_NFN as u64, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(done_nfn),
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

    // Slot 3: own TCB cap
    child_cnode
        .absolute_cptr_from_bits_with_depth(CHILD_SLOT_TCB as u64, CHILD_CNODE_SIZE_BITS)
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
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.7-A ===");
    sel4::debug_println!("    Topologie N=2 agents (ADR-0044)");

    let mut allocator = ObjectAllocator::new(bootinfo);
    let free_page_addr = init_free_page_addr(bootinfo);
    let cnode = sel4::init_thread::slot::CNODE.cap();

    // ── 1. Allouer les objets partagés ────────────────────────────────────────

    // Endpoint IPC partagé (1 seul endpoint, dispatch par badge ADR-0044 §D4)
    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

    // Notifications done : une par runtime
    let done_nfn_a = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();
    let done_nfn_b = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();

    // Ring A : frame physique + copie pour le server
    let ring_a_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
    let ring_a_server = copy_frame(cnode, ring_a_orig, &mut allocator);

    // Ring B : frame physique + copie pour le server
    let ring_b_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
    let ring_b_server = copy_frame(cnode, ring_b_orig, &mut allocator);

    // ── 2. Créer le VSpace du server ──────────────────────────────────────────
    // Le server reçoit les 2 rings : ring_a à _end+G, ring_b à _end+2G

    let server_image = File::parse(SERVER_ELF_CONTENTS).unwrap();
    let (server_vspace, server_ipc_buf_addr, server_ipc_buf_cap, _server_ring_vas) =
        create_child_vspace(
            &mut allocator,
            &server_image,
            sel4::init_thread::slot::VSPACE.cap(),
            free_page_addr,
            sel4::init_thread::slot::ASID_POOL.cap(),
            &[ring_a_server, ring_b_server],
        );

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

    let mut server_ctx = sel4::UserContext::default();
    *server_ctx.pc_mut() = server_image.entry().try_into().unwrap();
    server_tcb
        .tcb_write_all_registers(true, &mut server_ctx)
        .unwrap();

    sel4::debug_println!("[C7] server démarré");

    // ── 3. Spawner les 2 runtimes ─────────────────────────────────────────────

    let runtime_image = File::parse(RUNTIME_ELF_CONTENTS).unwrap();

    let _runtime_a_tcb = spawn_runtime(
        &mut allocator,
        &runtime_image,
        ring_a_orig,
        endpoint,
        done_nfn_a,
        AGENT_A_ID,
        free_page_addr,
    );
    sel4::debug_println!("[C7] runtime A (badge={}) démarré", AGENT_A_ID);

    let _runtime_b_tcb = spawn_runtime(
        &mut allocator,
        &runtime_image,
        ring_b_orig,
        endpoint,
        done_nfn_b,
        AGENT_B_ID,
        free_page_addr,
    );
    sel4::debug_println!("[C7] runtime B (badge={}) démarré", AGENT_B_ID);

    // ── 4. Attendre les signaux done des 2 runtimes ──────────────────────────

    done_nfn_a.wait();
    sel4::debug_println!("[C7] superviseur : done_A reçu");

    done_nfn_b.wait();
    sel4::debug_println!("[C7] superviseur : done_B reçu");

    sel4::debug_println!("C7-A_PASS");

    sel4::init_thread::suspend_self()
}
