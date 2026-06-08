(module
  ;; Import host function emit() sans arguments — le payload est géré côté host
  ;; pour éviter la mémoire WASM qui cause des réservations virtuelles massives
  (import "env" "emit" (func $emit (result i32)))

  ;; Pas de mémoire linéaire WASM (évite la réservation de 8GB par Wasmtime)

  ;; Fonction principale exportée
  (func (export "run")
    ;; Appelle emit
    call $emit
    drop
  )
)
