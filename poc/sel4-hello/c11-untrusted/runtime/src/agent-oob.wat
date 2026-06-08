(module
  (import "env" "emit" (func $emit (result i32)))
  ;; Pas de mémoire linéaire WASM (évite la réservation de 8GB par Wasmtime)
  ;; Le trap est déclenché par l'instruction `unreachable` — équivalent fonctionnel
  ;; d'un accès mémoire invalide pour le test P-alpha (isolation processus sous trap).
  (func (export "run")
    call $emit
    drop
    unreachable
  )
)
