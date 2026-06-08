// Jalon C.2 — Root task custom minimale (ADR-0039)
//
// Critères C.2 :
//   (a) Print custom sur UART seL4
//   (b) Accès à bootinfo (untyped_list)
//   (c) Retype 1 Untyped ≥ 4 KB → SmallPage (AArch64, 4 KB)
//   Signal de succès : "C2_PASS" → attendu par test.py
#![no_std]
#![no_main]

use sel4_root_task::{root_task, Never};

#[root_task]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.2 ===");
    sel4::debug_println!("    Cible : AArch64 QEMU virt, seL4 15.0.0 (ADR-0039)");

    // Critère C.2b : accès bootinfo + inventaire Untyped
    let untyped_list = bootinfo.untyped_list();
    sel4::debug_println!("bootinfo.untyped_list() : {} régions", untyped_list.len());

    // Trouver un Untyped non-device de taille ≥ 4 KB (size_bits ≥ 12, SmallPage = seL4_PageBits)
    let blueprint = sel4::ObjectBlueprint::Arch(sel4::ObjectBlueprintArch::SmallPage);
    let chosen_ix = bootinfo
        .untyped_list()
        .iter()
        .position(|desc| !desc.is_device() && desc.size_bits() >= blueprint.physical_size_bits())
        .expect("aucun Untyped disponible ≥ 4 KB");

    let desc = &untyped_list[chosen_ix];
    sel4::debug_println!(
        "Untyped[{}] : size_bits={} is_device={}",
        chosen_ix,
        desc.size_bits(),
        desc.is_device()
    );

    // Critère C.2c : retype → SmallPage (4 KB, AArch64)
    let untyped = bootinfo.untyped().index(chosen_ix).cap();

    let dest_slot = bootinfo
        .empty()
        .range()
        .map(sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index)
        .next()
        .expect("aucun slot CNode disponible");

    let cnode = sel4::init_thread::slot::CNODE.cap();

    untyped
        .untyped_retype(
            &blueprint,
            &cnode.absolute_cptr_for_self(),
            dest_slot.index(),
            1,
        )
        .map_err(|e| {
            sel4::debug_println!("ERREUR retype : {:?}", e);
            e
        })?;

    sel4::debug_println!("retype Untyped → SmallPage : OK");

    sel4::debug_println!("C2_PASS");

    sel4::init_thread::suspend_self()
}
