(module
  (import "env" "emit" (func $emit (result i32)))
  ;; Pas de mémoire linéaire (évite réservation 8 GB Wasmtime — L85)
  (func (export "run")
    call $emit
    drop
  )
)
