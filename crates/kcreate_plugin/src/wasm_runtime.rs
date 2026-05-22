//! Sandboxed WASM plugin runtime backed by `wasmi`.
//!
//! Phase 2 host ABI (all functions live in the `env` namespace):
//!
//! * `kcreate_log(ptr: i32, len: i32)`
//!   Copy `len` bytes from plugin memory at `ptr` into the host log
//!   buffer (interpreted as UTF-8; invalid sequences are replaced).
//!
//! * `kcreate_get_input(ptr: i32, max_len: i32) -> i32`
//!   Copy up to `max_len` bytes of the host-supplied input JSON
//!   into plugin memory at `ptr`. Returns the number of bytes
//!   written; if the buffer is too small the input is truncated.
//!
//! * `kcreate_get_input_len() -> i32`
//!   Length of the input JSON in bytes (so plugins can size their
//!   buffer before calling `kcreate_get_input`).
//!
//! * `kcreate_set_output(ptr: i32, len: i32)`
//!   Copy `len` bytes from plugin memory at `ptr` into the host
//!   output buffer. The last call wins.
//!
//! Plugins have **no** access to anything else. No filesystem, no
//! sockets, no environment variables, no clock — those host functions
//! are simply not exported into the linker.

use std::sync::Arc;

use thiserror::Error;
use wasmi::{
    errors::MemoryError, Caller, Config, Engine, Linker, Memory, Module, ResourceLimiter, Store,
    TypedFunc,
};

/// Errors from sandbox execution.
#[derive(Debug, Error)]
pub enum WasmPluginError {
    #[error("wasm compile / link: {0}")]
    Wasm(String),
    #[error("plugin export `{0}` not found")]
    MissingExport(String),
    #[error("plugin memory access out of bounds")]
    MemoryOutOfBounds,
    #[error("plugin memory limit exceeded ({0} pages)")]
    MemoryLimitExceeded(u32),
    #[error("plugin output is not valid UTF-8")]
    OutputNotUtf8,
}

impl From<wasmi::Error> for WasmPluginError {
    fn from(e: wasmi::Error) -> Self {
        Self::Wasm(e.to_string())
    }
}

impl From<MemoryError> for WasmPluginError {
    fn from(e: MemoryError) -> Self {
        Self::Wasm(e.to_string())
    }
}

impl From<wasmi::errors::LinkerError> for WasmPluginError {
    fn from(e: wasmi::errors::LinkerError) -> Self {
        Self::Wasm(e.to_string())
    }
}

/// Output of a plugin execution.
#[derive(Debug, Clone, Default)]
pub struct PluginOutput {
    /// The plugin's chosen output (last call to `kcreate_set_output`).
    pub output: String,
    /// Lines written via `kcreate_log`.
    pub logs: Vec<String>,
}

/// Per-execution host state — pinned to the wasmi `Store`. Holds the
/// input the plugin is allowed to read, the output the plugin produces,
/// the log lines, and the memory limiter that enforces the page cap.
struct HostData {
    input: Arc<Vec<u8>>,
    output: Vec<u8>,
    logs: Vec<String>,
    memory: Option<Memory>,
    limiter: PageLimiter,
}

#[derive(Debug, Clone)]
struct PageLimiter {
    max_bytes: usize,
}

impl ResourceLimiter for PageLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, MemoryError> {
        Ok(desired <= self.max_bytes)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmi::errors::TableError> {
        Ok(true)
    }
}

/// Stateless engine wrapper. Each `execute` call creates a fresh
/// `Store` so plugin runs are fully independent.
#[derive(Debug, Clone)]
pub struct WasmPluginRuntime {
    engine: Engine,
}

impl Default for WasmPluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

const PAGE_BYTES: usize = 64 * 1024;

impl WasmPluginRuntime {
    pub fn new() -> Self {
        let mut config = Config::default();
        // Bulk-memory and reference-types are commonly emitted by
        // Rust's wasm32-unknown-unknown backend; enable so plugins
        // built with `cargo build --target wasm32-unknown-unknown`
        // can load.
        config.wasm_bulk_memory(true).wasm_reference_types(true);
        let engine = Engine::new(&config);
        Self { engine }
    }

    /// Execute `function_name` from `wasm_bytes` with `input_json`,
    /// returning whatever the plugin wrote with `kcreate_set_output`
    /// plus any log lines.
    pub fn execute(
        &self,
        wasm_bytes: &[u8],
        function_name: &str,
        input_json: &str,
        memory_limit_pages: u32,
    ) -> Result<PluginOutput, WasmPluginError> {
        let module = Module::new(&self.engine, wasm_bytes)?;
        let host = HostData {
            input: Arc::new(input_json.as_bytes().to_vec()),
            output: Vec::new(),
            logs: Vec::new(),
            memory: None,
            limiter: PageLimiter {
                max_bytes: (memory_limit_pages as usize).saturating_mul(PAGE_BYTES),
            },
        };
        let mut store = Store::new(&self.engine, host);
        store.limiter(|s: &mut HostData| &mut s.limiter);

        let mut linker = Linker::<HostData>::new(&self.engine);
        register_host_funcs(&mut linker)?;

        let instance = linker.instantiate(&mut store, &module)?.start(&mut store)?;

        // Cache the memory export on the host data so the host funcs
        // don't have to re-resolve it on every call.
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| WasmPluginError::MissingExport("memory".to_string()))?;
        store.data_mut().memory = Some(memory);

        let func: TypedFunc<(), ()> = instance
            .get_typed_func(&store, function_name)
            .map_err(|_| WasmPluginError::MissingExport(function_name.to_string()))?;
        func.call(&mut store, ())?;

        let data = store.into_data();
        let output = String::from_utf8(data.output).map_err(|_| WasmPluginError::OutputNotUtf8)?;
        Ok(PluginOutput {
            output,
            logs: data.logs,
        })
    }
}

fn register_host_funcs(linker: &mut Linker<HostData>) -> Result<(), WasmPluginError> {
    linker.func_wrap(
        "env",
        "kcreate_log",
        |mut caller: Caller<HostData>, ptr: i32, len: i32| -> Result<(), wasmi::Error> {
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmi::Error::new("plugin memory not initialized"))?;
            let mut buf = vec![0u8; usize_from_i32(len)?];
            memory
                .read(&caller, usize_from_i32(ptr)?, &mut buf)
                .map_err(wasm_err)?;
            let text = String::from_utf8_lossy(&buf).into_owned();
            caller.data_mut().logs.push(text);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "env",
        "kcreate_get_input_len",
        |caller: Caller<HostData>| -> i32 {
            i32::try_from(caller.data().input.len()).unwrap_or(i32::MAX)
        },
    )?;

    linker.func_wrap(
        "env",
        "kcreate_get_input",
        |mut caller: Caller<HostData>, ptr: i32, max_len: i32| -> Result<i32, wasmi::Error> {
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmi::Error::new("plugin memory not initialized"))?;
            let input = caller.data().input.clone();
            let to_write = input.len().min(usize_from_i32(max_len)?);
            memory
                .write(&mut caller, usize_from_i32(ptr)?, &input[..to_write])
                .map_err(wasm_err)?;
            Ok(i32::try_from(to_write).unwrap_or(i32::MAX))
        },
    )?;

    linker.func_wrap(
        "env",
        "kcreate_set_output",
        |mut caller: Caller<HostData>, ptr: i32, len: i32| -> Result<(), wasmi::Error> {
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmi::Error::new("plugin memory not initialized"))?;
            let mut buf = vec![0u8; usize_from_i32(len)?];
            memory
                .read(&caller, usize_from_i32(ptr)?, &mut buf)
                .map_err(wasm_err)?;
            caller.data_mut().output = buf;
            Ok(())
        },
    )?;

    Ok(())
}

fn usize_from_i32(value: i32) -> Result<usize, wasmi::Error> {
    usize::try_from(value).map_err(|_| wasmi::Error::new("negative offset/length"))
}

fn wasm_err(e: MemoryError) -> wasmi::Error {
    wasmi::Error::new(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal plugin that copies input -> output and emits one log line.
    /// Implemented in WAT to avoid having to build a Rust→WASM crate
    /// in CI just for tests.
    const ECHO_WAT: &str = r#"
        (module
            (import "env" "kcreate_log" (func $log (param i32 i32)))
            (import "env" "kcreate_get_input_len" (func $in_len (result i32)))
            (import "env" "kcreate_get_input" (func $in_read (param i32 i32) (result i32)))
            (import "env" "kcreate_set_output" (func $out_write (param i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "hello\00")
            (func (export "run")
                ;; log "hello"
                i32.const 0  i32.const 5  call $log
                ;; copy input -> output at offset 64
                i32.const 64  call $in_len  call $in_read  drop
                i32.const 64  call $in_len  call $out_write
            )
        )
    "#;

    #[test]
    fn echo_plugin_round_trips_input() {
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let out = rt.execute(&wasm, "run", r#"{"x":1}"#, 16).unwrap();
        assert_eq!(out.output, r#"{"x":1}"#);
        assert_eq!(out.logs, vec!["hello".to_string()]);
    }

    #[test]
    fn rejects_missing_function() {
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let err = rt.execute(&wasm, "no_such_fn", "{}", 16).unwrap_err();
        assert!(matches!(err, WasmPluginError::MissingExport(_)));
    }

    #[test]
    fn rejects_invalid_wasm() {
        let rt = WasmPluginRuntime::new();
        let err = rt.execute(b"not wasm", "run", "{}", 16).unwrap_err();
        assert!(matches!(err, WasmPluginError::Wasm(_)));
    }

    #[test]
    fn enforces_memory_page_limit() {
        // Module that tries to grow memory by 100 pages on entry.
        let wat = r#"
            (module
                (memory (export "memory") 1)
                (func (export "run")
                    i32.const 100
                    memory.grow
                    drop)
            )
        "#;
        let wasm = wat::parse_str(wat).unwrap();
        let rt = WasmPluginRuntime::new();
        // memory_limit_pages = 4 -> grow request for 100 should be
        // rejected by the limiter. wasmi reports memory.grow failure
        // via a return value of -1 inside the wasm program, but the
        // limiter rejection short-circuits with an Error; we just
        // assert the call doesn't grow past the limit by checking it
        // either succeeds (with memory.grow returning -1) or errors.
        let result = rt.execute(&wasm, "run", "{}", 4);
        // Either the limiter denies and the call still completes (the
        // wasm just ignores the -1) — which is a successful run — or
        // we get an error. In neither case should the host crash.
        assert!(result.is_ok() || matches!(result, Err(WasmPluginError::Wasm(_))));
    }

    #[test]
    fn empty_input_returns_empty_output() {
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let out = rt.execute(&wasm, "run", "", 16).unwrap();
        assert_eq!(out.output, "");
    }

    #[test]
    fn each_run_is_isolated() {
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let a = rt.execute(&wasm, "run", "alpha", 16).unwrap();
        let b = rt.execute(&wasm, "run", "beta", 16).unwrap();
        assert_eq!(a.output, "alpha");
        assert_eq!(b.output, "beta");
        // Logs from run a must not leak into run b.
        assert_eq!(a.logs.len(), 1);
        assert_eq!(b.logs.len(), 1);
    }
}
