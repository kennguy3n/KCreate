;; hello.wat — minimal KCreate plugin.
;;
;; Logs "Hello from WASM" once, then sets the plugin output to
;; "hello-output". Uses only the basic ABI (no permissions required).
;;
;; Compile with:
;;     wat2wasm hello.wat -o hello.wasm
;; or any equivalent WAT → WASM tool.
(module
  (import "env" "kcreate_log"        (func $log  (param i32 i32)))
  (import "env" "kcreate_set_output" (func $out  (param i32 i32)))

  (memory (export "memory") 1)

  ;;   0..15  : "Hello from WASM"   (15 bytes)
  ;;  16..28  : "hello-output"      (12 bytes)
  (data (i32.const 0)  "Hello from WASM")
  (data (i32.const 16) "hello-output")

  (func (export "run")
    ;; kcreate_log(ptr=0, len=15)
    i32.const 0
    i32.const 15
    call $log

    ;; kcreate_set_output(ptr=16, len=12)
    i32.const 16
    i32.const 12
    call $out
  )
)
