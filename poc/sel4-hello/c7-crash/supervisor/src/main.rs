//! Jalon C.7-crash — Root task superviseur
//!
//! Topologie N=2 agents (ADR-0044) :
//!   - Server (VSpace B) : reçoit commits de 2 agents, oracle badge=0xC7FE
//!   - Runtime A (VSpace A1) : badge=AGENT_A_ID, KILL_POINT configuré (KP1-4)
//!   - Runtime B (VSpace A2) : badge=AGENT_B_ID, KILL_POINT=0 (nominal)
//!
//! Après suspension des 2 runtimes : oracle query → asserter I3-N + I4 (ADR-0044).
//!
//! Critère de sortie : C7-crash_PASS (4 KPs, un par run)
//! Signal de succès : KP{N}_PASS + C7-crash_PASS (dernier run)

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

// ELFs embarqués — chemins fournis par build.rs
const SERVER_ELF_CONTENTS: &[u8] = include_bytes!(env!("SERVER_ELF"));
const RUNTIME_A_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_A_ELF"));
const RUNTIME_B_ELF_CONTENTS: &[u8] = include_bytes!(env!("RUNTIME_B_ELF"));

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

// Badges agents (ADR-0044 §D2)
const AGENT_A_ID: sel4::Badge = 1;
const AGENT_B_ID: sel4::Badge = 2;
const ORACLE_BADGE: sel4::Badge = 0xC7FE;

// child_cnode_size_bits = 2 → 4 slots par CNode enfant (slots 0-3)
const CHILD_CNODE_SIZE_BITS: usize = 2;

// Slots dans les CNodes enfants
// Slot 0: NULL
// Slot 1: commit-cap (endpoint, badge=agent_id, CapRights::all() — GrantReply requis L70)
// Slot 2: suspend_nfn (write_only) — signal avant self-suspend (KP=0: fin nominale)
// Slot 3: own TCB cap (pour tcb_suspend par le runtime lui-même)
const CHILD_SLOT_EP: u64 = 1;
const CHILD_SLOT_NFN: u64 = 2;
const CHILD_SLOT_TCB: u64 = 3;

// Kill point connu du superviseur (pour assertions I3-N)
// Valeur injectée par le Makefile via env var KILL_POINT
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
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.7-crash ===");
    sel4::debug_println!("    KILL_POINT={} (ADR-0044)", KILL_POINT);

    let mut allocator = ObjectAllocator::new(bootinfo);
    let free_page_addr = init_free_page_addr(bootinfo);
    let cnode = sel4::init_thread::slot::CNODE.cap();

    // ── 1. Allouer les objets partagés ────────────────────────────────────────

    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

    // Notifications suspend : une par runtime (signal avant tcb_suspend)
    let suspend_nfn_a = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();
    let suspend_nfn_b = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();

    // Ring A + copie server
    let ring_a_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
    let ring_a_server = copy_frame(cnode, ring_a_orig, &mut allocator);

    // Ring B + copie server
    let ring_b_orig = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
    let ring_b_server = copy_frame(cnode, ring_b_orig, &mut allocator);

    // Oracle cap (superviseur → serveur, badge=ORACLE_BADGE)
    let oracle_ep_slot = allocator.next_slot();
    cnode
        .absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(oracle_ep_slot)
                .cptr(),
        )
        .mint(
            &cnode.absolute_cptr(endpoint),
            sel4::CapRights::all(),
            ORACLE_BADGE,
        )
        .unwrap();
    let oracle_ep: sel4::cap::Endpoint =
        sel4::init_thread::Slot::from_index(oracle_ep_slot).cap();

    // ── 2. Créer le VSpace du server ──────────────────────────────────────────

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

    let server_cnode =
        allocator.allocate_variable_sized::<sel4::cap_type::CNode>(CHILD_CNODE_SIZE_BITS);

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
    server_tcb.tcb_write_all_registers(true, &mut server_ctx).unwrap();
    sel4::debug_println!("[C7-crash] server démarré");

    // ── 3. Spawner runtime A (instrumenté KP={}) et runtime B (nominal) ───────

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
    sel4::debug_println!("[C7-crash] runtime A (badge={}, KP={}) démarré", AGENT_A_ID, KILL_POINT);

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
    sel4::debug_println!("[C7-crash] runtime B (badge={}, nominal) démarré", AGENT_B_ID);

    // ── 4. Attendre les signaux suspend des 2 runtimes ────────────────────────
    // Les 2 runtimes signalent avant de se suspendre (via tcb_suspend)

    suspend_nfn_a.wait();
    sel4::debug_println!("[C7-crash] superviseur : suspend_A reçu (KP={})", KILL_POINT);

    suspend_nfn_b.wait();
    sel4::debug_println!("[C7-crash] superviseur : suspend_B reçu");

    // ── 5. Oracle query → asserter I3-N + I4 ─────────────────────────────────
    // oracle_ep.call() → serveur répond (seq_a, seq_b)

    let oracle_reply = oracle_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

    let (seq_a, seq_b) = sel4::with_ipc_buffer_mut(|buf| {
        let a = buf.msg_regs()[0];
        let b = buf.msg_regs()[1];
        (a, b)
    });

    sel4::debug_println!(
        "[C7-crash] oracle: seq_a={}, seq_b={} (KP={})",
        seq_a, seq_b, KILL_POINT
    );
    let _ = oracle_reply;

    // Assertions I3-N (ADR-0044) :
    //   K = 1 (1 action par agent)
    //   KP1/KP2/KP3 ⇒ seq_a = 0 (k-1)
    //   KP4 ⇒ seq_a = 1 (k)
    let expected_seq_a: u64 = if KILL_POINT == 4 { 1 } else { 0 };

    // Assertion I4 (non-interférence d'intégrité) :
    //   seq_b = 1 (B a commité 1 action, non affecté par le crash de A)
    let expected_seq_b: u64 = 1;

    let i3n_ok = seq_a == expected_seq_a;
    let i4_ok = seq_b == expected_seq_b;

    sel4::debug_println!(
        "[C7-crash] I3-N: seq_a={} expected={} → {}",
        seq_a, expected_seq_a,
        if i3n_ok { "OK" } else { "FAIL" }
    );
    sel4::debug_println!(
        "[C7-crash] I4: seq_b={} expected={} → {}",
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
