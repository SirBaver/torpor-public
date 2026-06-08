//! Jalon C.6-crash — Root task superviseur
//!
//! Topologie 2-processus :
//!   - Server (VSpace B) : recv endpoint → lit ring → commit Q3-C en RAM → reply
//!   - Runtime (VSpace A) : WASM emit() → kill_point → suspend_nfn.signal() → tcb_suspend()
//!
//! Après la suspension du runtime, le superviseur envoie une oracle query
//! au serveur (badge=0xCAFE) pour récupérer (journal_len, blob_count, header_count).
//! Il vérifie les invariants I1/I2/I3 et imprime KPx_PASS ou KPx_FAIL.
//!
//! KILL_POINT et EXPECTED_N sont connus à compile time.

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

// KILL_POINT — aligné sur le runtime (même env var)
// Note: match sur &str en const context non supporté dans nightly-2026-03-18
const fn parse_kill_point(s: Option<&'static str>) -> u32 {
    match s {
        None => 1, // défaut : KP1 si non défini
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

// EXPECTED_N : journal_len attendu après le kill point
// KP1/KP2/KP3 ⇒ commit pas encore reçu par le serveur ⇒ n = 0
// KP4 ⇒ ep.call() a retourné = commit fait ⇒ n = 1
const EXPECTED_N: u64 = if KILL_POINT == 4 { 1 } else { 0 };

// Badge oracle — doit correspondre à la constante dans server/src/main.rs
const ORACLE_BADGE: sel4::Badge = 0xCAFE;

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
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.6-crash ===");
    sel4::debug_println!("    KILL_POINT={} EXPECTED_N={}", KILL_POINT, EXPECTED_N);

    let mut allocator = ObjectAllocator::new(bootinfo);
    let free_page_addr = init_free_page_addr(bootinfo);

    // ── 1. Allouer les objets partagés ────────────────────────────────────────

    // Endpoint IPC entre runtime (caller) et server (receiver)
    let endpoint = allocator.allocate_fixed_sized::<sel4::cap_type::Endpoint>();

    // Notification suspend : runtime → superviseur (signal avant self-suspend)
    let suspend_nfn = allocator.allocate_fixed_sized::<sel4::cap_type::Notification>();

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

    // Oracle endpoint : mint de endpoint avec badge ORACLE_BADGE
    // Le superviseur utilise cette cap badgée pour distinguer oracle vs commit dans le serveur
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
    // Le server reçoit badge=0 (commits normaux) et badge=0xCAFE (oracle queries)
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

    // Slot 2: own TCB cap du server (cohérence avec C.6-integration)
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

    sel4::debug_println!("[C6-crash] server démarré");

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

    // Slot 2: suspend_nfn (write_only) — runtime signale avant de se suspendre
    runtime_cnode
        .absolute_cptr_from_bits_with_depth(2, CHILD_CNODE_SIZE_BITS)
        .mint(
            &cnode.absolute_cptr(suspend_nfn),
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

    // Slot 3: own TCB cap du runtime (pour self-suspend dans self_suspend_at())
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

    sel4::debug_println!("[C6-crash] runtime démarré");

    // ── 4. Attendre que le runtime se suspend ─────────────────────────────────
    // Le runtime émet suspend_nfn.signal() juste avant tcb_suspend()

    suspend_nfn.wait();
    sel4::debug_println!("[C6-crash] superviseur : runtime suspendu au KP{}", KILL_POINT);

    // ── 5. Oracle query au serveur ────────────────────────────────────────────
    // Utiliser oracle_ep (badge=0xCAFE) pour distinguer l'oracle d'un commit normal

    oracle_ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

    let (journal_len, blob_count, header_count) = sel4::with_ipc_buffer(|buf| {
        (
            buf.msg_regs()[0],
            buf.msg_regs()[1],
            buf.msg_regs()[2],
        )
    });

    sel4::debug_println!(
        "[C6-crash] oracle: journal={}, blobs={}, headers={}, expected_n={}",
        journal_len,
        blob_count,
        header_count,
        EXPECTED_N
    );

    // ── 6. Vérification des invariants ────────────────────────────────────────
    // I1 : journal_len ∈ {0, 1} (pas d'entrée partielle)
    // I2 : si journal_len > 0, headers_count >= journal_len et blobs_count >= journal_len
    // I3 : journal_len == EXPECTED_N (KP1/KP2/KP3 ⇒ 0 ; KP4 ⇒ 1)

    let i1_ok = journal_len <= 1;
    let i2_ok = if journal_len == 0 {
        true
    } else {
        header_count >= journal_len && blob_count >= journal_len
    };
    let i3_ok = journal_len == EXPECTED_N;

    if i1_ok && i2_ok && i3_ok {
        sel4::debug_println!("KP{}_PASS", KILL_POINT);
    } else {
        sel4::debug_println!(
            "KP{}_FAIL I1={} I2={} I3={} (journal={}, blobs={}, headers={}, expected={})",
            KILL_POINT,
            i1_ok,
            i2_ok,
            i3_ok,
            journal_len,
            blob_count,
            header_count,
            EXPECTED_N
        );
    }

    // Suspendre le thread init
    sel4::init_thread::suspend_self()
}
