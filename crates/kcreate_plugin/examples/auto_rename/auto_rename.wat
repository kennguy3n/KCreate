;; auto_rename.wat — extended ABI demo: submit one update_node proposal.
;;
;; Requires both `read_document` and `write_document`. The plugin
;; submits a single `update_node` proposal that renames the node with
;; the placeholder UUID to "Renamed".
;;
;; By default the placeholder is the all-zero UUID, which doesn't
;; resolve to a real node. The host therefore rejects the proposal —
;; you'll see one entry in the return envelope's `proposals` array
;; with `outcome.status == "rejected"`. That's intentional: it proves
;; the host validates proposals against the live document graph.
;;
;; To actually rename a node, replace the UUID inside the data section
;; below with a real id (e.g. from `kcreate_read_document` with
;; `{"type":"list_nodes"}`) and recompile.
;;
;; Compile with:
;;     wat2wasm auto_rename.wat -o auto_rename.wasm
(module
  (import "env" "kcreate_log"
    (func $log (param i32 i32)))
  (import "env" "kcreate_write_proposal"
    (func $propose (param i32 i32) (result i32)))

  (memory (export "memory") 1)

  ;; The full proposal JSON, 100 bytes. The `\"` escapes are WAT
  ;; syntax — the bytes that land in memory are the un-escaped JSON.
  (data (i32.const 0)
    "{\"type\":\"update_node\",\"node_id\":\"00000000-0000-0000-0000-000000000000\",\"changes\":{\"name\":\"Renamed\"}}")

  ;; Log message: "auto-rename: submitting proposal"  (32 bytes)
  (data (i32.const 128) "auto-rename: submitting proposal")

  (func (export "run")
    i32.const 128
    i32.const 32
    call $log

    ;; kcreate_write_proposal(ptr=0, len=100). We drop the return
    ;; value; the host reports applied/rejected status in its return
    ;; envelope to the caller, not back to the plugin.
    i32.const 0
    i32.const 100
    call $propose
    drop
  )
)
