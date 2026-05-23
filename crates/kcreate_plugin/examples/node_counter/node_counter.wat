;; node_counter.wat — extended ABI demo: list every node id.
;;
;; Calls `kcreate_read_document` with `{"type":"list_nodes"}`. The host
;; writes the JSON response (an array of UUID strings) into the
;; plugin's output buffer, so by the time `run` returns the plugin has
;; nothing else to do — the JSON array is already the plugin's output.
;;
;; Requires `read_document` permission.
;;
;; Compile with:
;;     wat2wasm node_counter.wat -o node_counter.wasm
(module
  (import "env" "kcreate_read_document"
    (func $read_doc (param i32 i32) (result i32)))
  (import "env" "kcreate_log"
    (func $log (param i32 i32)))

  (memory (export "memory") 1)

  ;; Query: {"type":"list_nodes"}  (21 bytes)
  (data (i32.const 0)
    "{\"type\":\"list_nodes\"}")

  ;; Log message: "counting nodes"  (14 bytes)
  (data (i32.const 64) "counting nodes")

  (func (export "run")
    ;; Tell the user we're running.
    i32.const 64
    i32.const 14
    call $log

    ;; kcreate_read_document(ptr=0, len=21). The host writes the JSON
    ;; response (the array of ids) directly into our output buffer; we
    ;; drop the returned length because we don't need it on the WASM
    ;; side — the bridge returns the output to the renderer verbatim.
    i32.const 0
    i32.const 21
    call $read_doc
    drop
  )
)
