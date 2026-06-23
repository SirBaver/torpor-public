// ILLUSTRATION DU PRINCIPE — CE N'EST PAS LE RUNTIME TORPOR.
// Reproduit l'IDÉE de l'adressage par contenu + tamper-evidence en autonome
// (sans RocksDB / Wasmtime / le DAG réel). La PREUVE vit dans ../REPRODUCE.md
// (le vrai système, cloné au tag, mesuré sur Linux/RocksDB). Aucune borne chiffrée ici.

use sha2::{Digest, Sha256};

/// Une « action » minimale : un contenu + ses parents causaux (par id).
struct DemoAction {
    content: Vec<u8>,
    caused_by: Vec<String>,
}

/// L'identifiant EST le hash du contenu (+ parents). Changez un octet → l'id change.
fn content_id(a: &DemoAction) -> String {
    let mut h = Sha256::new();
    h.update(&a.content);
    for p in &a.caused_by {
        h.update(p.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    // « analyse » → « correctif » : le correctif est causé par l'analyse.
    let analyse = DemoAction { content: b"analyse: incident X".to_vec(), caused_by: vec![] };
    let id_analyse = content_id(&analyse);

    let correctif = DemoAction {
        content: b"correctif: patch Y".to_vec(),
        caused_by: vec![id_analyse.clone()],
    };
    let id_correctif = content_id(&correctif);

    println!("analyse   id = {id_analyse}");
    println!("correctif id = {id_correctif}  (caused_by = [{id_analyse}])");

    // Falsification : on change le contenu de l'analyse après coup.
    let analyse_falsifiee =
        DemoAction { content: b"analyse: incident Z".to_vec(), caused_by: vec![] };
    let id_recalcule = content_id(&analyse_falsifiee);

    println!("\n-- falsification de « analyse » --");
    println!("id stocke    = {id_analyse}");
    println!("id recalcule = {id_recalcule}");
    assert_ne!(id_analyse, id_recalcule, "le hash doit changer si le contenu change");

    // Le correctif pointe vers id_analyse, désormais introuvable → orphelin détecté.
    let cible = &correctif.caused_by[0];
    let orphelin = *cible != id_recalcule;
    println!("le correctif pointe {cible}");
    println!("orphelin detecte ? {orphelin}");
    assert!(orphelin, "la falsification doit rendre le lien causal orphelin");

    println!("\nOK : falsification detectable (id != hash) et lien causal casse.");
}
