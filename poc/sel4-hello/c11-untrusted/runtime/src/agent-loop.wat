(module
  (import "env" "emit" (func $emit (result i32)))
  (import "env" "started" (func $started))
  ;; Pas de mémoire linéaire WASM (évite la réservation de 8GB par Wasmtime)
  (func (export "run")
    call $emit
    drop
    call $started
    block $break
      loop $top
        br $top
      end
    end
  )
)
