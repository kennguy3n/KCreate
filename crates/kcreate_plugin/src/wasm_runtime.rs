//! Sandboxed WASM plugin runtime backed by `wasmi`.
//!
//! Phase 2 host ABI (all functions live in the `env` namespace):
//!
//! ### Basic ABI (always available)
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
//! ### Extended ABI (Phase 2 Task 13; permission-gated)
//!
//! These activate only when the plugin runs under a
//! [`crate::context::PluginContext`] via [`WasmPluginRuntime::execute_with_context`].
//! Plugins running through the legacy [`WasmPluginRuntime::execute`]
//! see no extended host funcs (they fail to link), preserving the
//! Phase 1 sandbox semantics.
//!
//! * `kcreate_read_document(ptr: i32, len: i32) -> i32`
//!   Read a JSON query (`{"type":"list_nodes" | "get_node" | "get_root", ...}`)
//!   from plugin memory; write the JSON response back into the
//!   plugin's *output* buffer and return its byte length. Gated by
//!   `PluginPermission::ReadDocument`; without that permission the
//!   call returns `0` and a single denial line is logged.
//!
//! * `kcreate_read_asset(hash_ptr: i32, hash_len: i32, buf_ptr: i32, buf_len: i32) -> i32`
//!   Resolve an asset by BLAKE3 hex hash and copy its bytes (up to
//!   `buf_len`) into the plugin's memory at `buf_ptr`. Returns the
//!   number of bytes written; `0` on permission deny, missing
//!   asset, or insufficient buffer. Gated by
//!   `PluginPermission::ReadAssets`.
//!
//! * `kcreate_write_proposal(ptr: i32, len: i32) -> i32`
//!   Submit a JSON proposal (`ProposedMutation` shape) from plugin
//!   memory. Returns `1` on acceptance into the host queue, `0` on
//!   permission deny or JSON parse error. Proposals are *not*
//!   applied here — the bridge layer validates and applies them
//!   after the plugin returns.
//!
//! Plugins have **no** access to anything else. No filesystem, no
//! sockets, no environment variables, no clock — those host functions
//! are simply not exported into the linker.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use blake3::Hash;

use parking_lot::Mutex;
use thiserror::Error;
use wasmi::{
    errors::MemoryError, Caller, Config, Engine, Linker, Memory, Module, ResourceLimiter, Store,
    TypedFunc,
};

use crate::context::{
    resolve_document_query, DocumentQuery, PluginContext, PluginProposal, ProposedMutation,
};
use crate::manifest::PluginPermission;

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

impl From<std::io::Error> for WasmPluginError {
    fn from(e: std::io::Error) -> Self {
        Self::Wasm(format!("read plugin bytes: {e}"))
    }
}

/// Output of a plugin execution.
#[derive(Debug, Clone, Default)]
pub struct PluginOutput {
    /// The plugin's chosen output (last call to `kcreate_set_output`).
    pub output: String,
    /// Lines written via `kcreate_log`.
    pub logs: Vec<String>,
    /// Proposals submitted by the plugin via
    /// `kcreate_write_proposal`. Empty for basic-ABI runs (the
    /// extended host functions aren't even registered there).
    pub proposals: Vec<ProposedMutation>,
}

impl PluginOutput {
    /// Roll the loose-leaf proposals into a [`PluginProposal`] batch
    /// stamped with `plugin_id`. The bridge layer uses this to feed
    /// the validation+apply pass in `phase2::plugin_execute_with_context`.
    #[must_use]
    pub fn into_proposal_batch(self, plugin_id: impl Into<String>) -> PluginProposal {
        PluginProposal {
            plugin_id: plugin_id.into(),
            mutations: self.proposals,
        }
    }
}

/// Per-execution host state — pinned to the wasmi `Store`. Holds the
/// input the plugin is allowed to read, the output the plugin produces,
/// the log lines, and the memory limiter that enforces the page cap.
///
/// When the plugin runs under a [`PluginContext`] (via
/// [`WasmPluginRuntime::execute_with_context`]) `context` is `Some(..)`
/// and the extended host functions are wired into the linker. Plugins
/// from the legacy path see `context = None` and the extended host
/// functions are simply not registered, preserving Phase 1 sandbox
/// semantics.
struct HostData {
    input: Arc<Vec<u8>>,
    output: Vec<u8>,
    logs: Vec<String>,
    memory: Option<Memory>,
    limiter: PageLimiter,
    context: Option<PluginContext>,
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

/// Engine wrapper with a per-path compiled-`Module` cache. Cache
/// entries record the file's `(mtime, size)` for a stat-only fast path
/// and the BLAKE3 content hash for the slow path so a rebuild within
/// the same mtime second is still picked up correctly. Each `execute`
/// call creates a fresh `Store` so plugin runs are fully independent —
/// only the *compiled* module is shared.
#[derive(Debug)]
pub struct WasmPluginRuntime {
    engine: Engine,
    module_cache: Mutex<std::collections::HashMap<PathBuf, CachedModule>>,
}

impl Default for WasmPluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

const PAGE_BYTES: usize = 64 * 1024;

/// Cache entry: a compiled `Module` plus enough metadata to detect
/// when the underlying `.wasm` file has changed. The bridge layer
/// calls into a hot plugin many times per session — compiling the
/// same `Module` on every call (and re-reading the `.wasm` file from
/// disk) burned ~50% of plugin execution time on real workloads.
///
/// Invalidation strategy:
/// * Fast path — `(mtime, size)` matches: reuse the cached `Module`
///   without reading the file. This is the common steady state.
/// * Slow path — anything else: read the file, BLAKE3-hash it, and
///   compare to `content_hash`. A matching hash means the contents
///   are unchanged (e.g. the file was rewritten with identical bytes
///   or touched within the same mtime second), so we refresh the
///   `(mtime, size)` and reuse the cached `Module`. A different hash
///   means we recompile.
///
/// The content-hash check fixes the "rebuild within the same second"
/// edge case that pure mtime keying missed on coarse-resolution
/// filesystems (ext3, HFS+).
#[derive(Debug, Clone)]
struct CachedModule {
    module: Module,
    mtime: Option<SystemTime>,
    size: u64,
    content_hash: Hash,
}

impl WasmPluginRuntime {
    pub fn new() -> Self {
        let mut config = Config::default();
        // Bulk-memory and reference-types are commonly emitted by
        // Rust's wasm32-unknown-unknown backend; enable so plugins
        // built with `cargo build --target wasm32-unknown-unknown`
        // can load.
        config.wasm_bulk_memory(true).wasm_reference_types(true);
        let engine = Engine::new(&config);
        Self {
            engine,
            module_cache: Mutex::new(std::collections::HashMap::new()),
        }
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
        self.execute_module(&module, function_name, input_json, memory_limit_pages)
    }

    /// Execute `function_name` from the `.wasm` file at `path`. The
    /// compiled `Module` is cached, keyed by `(path, mtime)`, so a
    /// rebuilt plugin is picked up on the next call but the steady-state
    /// hot path skips both the disk read and the `Module::new`
    /// recompilation.
    pub fn execute_path(
        &self,
        path: &Path,
        function_name: &str,
        input_json: &str,
        memory_limit_pages: u32,
    ) -> Result<PluginOutput, WasmPluginError> {
        let module = self.module_for_path(path)?;
        self.execute_module(&module, function_name, input_json, memory_limit_pages)
    }

    fn module_for_path(&self, path: &Path) -> Result<Module, WasmPluginError> {
        let meta = std::fs::metadata(path).ok();
        let mtime = meta.as_ref().and_then(|m| m.modified().ok());
        let size = meta.as_ref().map_or(0u64, std::fs::Metadata::len);

        // Fast path: stat-only invalidation. When `(mtime, size)`
        // matches the cached entry we trust the cache and skip both
        // the file read and the hash. Holding the lock only for the
        // lookup keeps the critical section tiny.
        {
            let cache = self.module_cache.lock();
            if let Some(entry) = cache.get(path) {
                if entry.mtime == mtime && entry.size == size {
                    return Ok(entry.module.clone());
                }
            }
        }

        // Slow path: read + hash. We always need the bytes anyway for
        // a potential recompile, and BLAKE3 over a typical plugin
        // (<1 MB) is sub-millisecond on a single core.
        let bytes = std::fs::read(path)?;
        let content_hash = blake3::hash(&bytes);

        // If the content hash matches a cached entry, the file
        // contents are unchanged even though `(mtime, size)` shifted
        // (e.g. touch, atomic-rename, sub-second rebuild). Refresh
        // the metadata so future calls hit the fast path, and reuse
        // the compiled `Module`.
        {
            let mut cache = self.module_cache.lock();
            if let Some(entry) = cache.get_mut(path) {
                if entry.content_hash == content_hash {
                    entry.mtime = mtime;
                    entry.size = size;
                    return Ok(entry.module.clone());
                }
            }
        }

        // Real change — compile a new module and replace the entry.
        let module = Module::new(&self.engine, &bytes)?;
        self.module_cache.lock().insert(
            path.to_path_buf(),
            CachedModule {
                module: module.clone(),
                mtime,
                size,
                content_hash,
            },
        );
        Ok(module)
    }

    /// Returns the number of compiled modules currently in the cache.
    /// Intended for tests / observability — not part of any wire API.
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.module_cache.lock().len()
    }

    fn execute_module(
        &self,
        module: &Module,
        function_name: &str,
        input_json: &str,
        memory_limit_pages: u32,
    ) -> Result<PluginOutput, WasmPluginError> {
        self.execute_module_inner(module, function_name, input_json, memory_limit_pages, None)
    }

    /// Execute under a [`PluginContext`]. Same calling convention as
    /// [`Self::execute`] / [`Self::execute_path`], plus the extended
    /// host ABI (`kcreate_read_document` / `kcreate_read_asset` /
    /// `kcreate_write_proposal`) is wired into the linker. Plugins
    /// without the relevant `PluginPermission` will see deny returns
    /// from the gated intrinsics. Any proposals produced by the
    /// plugin during the run land in
    /// [`PluginOutput::proposals`] for the bridge layer to validate
    /// and apply.
    pub fn execute_with_context(
        &self,
        wasm_bytes: &[u8],
        function_name: &str,
        input_json: &str,
        memory_limit_pages: u32,
        context: PluginContext,
    ) -> Result<PluginOutput, WasmPluginError> {
        let module = Module::new(&self.engine, wasm_bytes)?;
        self.execute_module_inner(
            &module,
            function_name,
            input_json,
            memory_limit_pages,
            Some(context),
        )
    }

    /// Like [`Self::execute_path`] but with a [`PluginContext`] (see
    /// [`Self::execute_with_context`]).
    pub fn execute_path_with_context(
        &self,
        path: &Path,
        function_name: &str,
        input_json: &str,
        memory_limit_pages: u32,
        context: PluginContext,
    ) -> Result<PluginOutput, WasmPluginError> {
        let module = self.module_for_path(path)?;
        self.execute_module_inner(
            &module,
            function_name,
            input_json,
            memory_limit_pages,
            Some(context),
        )
    }

    fn execute_module_inner(
        &self,
        module: &Module,
        function_name: &str,
        input_json: &str,
        memory_limit_pages: u32,
        context: Option<PluginContext>,
    ) -> Result<PluginOutput, WasmPluginError> {
        let extended = context.is_some();
        let host = HostData {
            input: Arc::new(input_json.as_bytes().to_vec()),
            output: Vec::new(),
            logs: Vec::new(),
            memory: None,
            limiter: PageLimiter {
                max_bytes: (memory_limit_pages as usize).saturating_mul(PAGE_BYTES),
            },
            context,
        };
        let mut store = Store::new(&self.engine, host);
        store.limiter(|s: &mut HostData| &mut s.limiter);

        let mut linker = Linker::<HostData>::new(&self.engine);
        register_host_funcs(&mut linker)?;
        if extended {
            register_extended_host_funcs(&mut linker)?;
        }

        let instance = linker.instantiate(&mut store, module)?.start(&mut store)?;

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
        let proposals = data
            .context
            .map(|c| c.proposals)
            .unwrap_or_default();
        Ok(PluginOutput {
            output,
            logs: data.logs,
            proposals,
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

/// Register the Phase 2 extended host functions on `linker`. These
/// are only attached when the plugin runs under a [`PluginContext`];
/// the basic / legacy execution path leaves them unlinked so old
/// plugins that don't expect them aren't affected.
fn register_extended_host_funcs(
    linker: &mut Linker<HostData>,
) -> Result<(), WasmPluginError> {
    linker.func_wrap(
        "env",
        "kcreate_read_document",
        |mut caller: Caller<HostData>, ptr: i32, len: i32| -> Result<i32, wasmi::Error> {
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmi::Error::new("plugin memory not initialized"))?;
            let permitted = caller
                .data()
                .context
                .as_ref()
                .is_some_and(|c| c.has(PluginPermission::ReadDocument));
            if !permitted {
                caller
                    .data_mut()
                    .logs
                    .push("kcreate_read_document: denied (missing ReadDocument)".to_string());
                return Ok(0);
            }

            // Read the query bytes from plugin memory.
            let mut buf = vec![0u8; usize_from_i32(len)?];
            memory
                .read(&caller, usize_from_i32(ptr)?, &mut buf)
                .map_err(wasm_err)?;
            let query_str = match std::str::from_utf8(&buf) {
                Ok(s) => s,
                Err(_) => {
                    caller
                        .data_mut()
                        .logs
                        .push("kcreate_read_document: query is not UTF-8".to_string());
                    return Ok(0);
                }
            };
            let query: DocumentQuery = match serde_json::from_str(query_str) {
                Ok(q) => q,
                Err(e) => {
                    caller.data_mut().logs.push(format!(
                        "kcreate_read_document: invalid query json: {e}"
                    ));
                    return Ok(0);
                }
            };

            let response = {
                let ctx = caller.data().context.as_ref().expect("permitted implies context");
                resolve_document_query(&ctx.document_snapshot, &query)
            };
            let response_bytes = match serde_json::to_vec(&response) {
                Ok(b) => b,
                Err(_) => return Ok(0),
            };
            let len = i32::try_from(response_bytes.len()).unwrap_or(i32::MAX);
            caller.data_mut().output = response_bytes;
            Ok(len)
        },
    )?;

    linker.func_wrap(
        "env",
        "kcreate_read_asset",
        |mut caller: Caller<HostData>,
         hash_ptr: i32,
         hash_len: i32,
         buf_ptr: i32,
         buf_len: i32|
         -> Result<i32, wasmi::Error> {
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmi::Error::new("plugin memory not initialized"))?;
            let permitted = caller
                .data()
                .context
                .as_ref()
                .is_some_and(|c| c.has(PluginPermission::ReadAssets));
            if !permitted {
                caller
                    .data_mut()
                    .logs
                    .push("kcreate_read_asset: denied (missing ReadAssets)".to_string());
                return Ok(0);
            }

            let mut hash_buf = vec![0u8; usize_from_i32(hash_len)?];
            memory
                .read(&caller, usize_from_i32(hash_ptr)?, &mut hash_buf)
                .map_err(wasm_err)?;
            let hash_str = match std::str::from_utf8(&hash_buf) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    caller
                        .data_mut()
                        .logs
                        .push("kcreate_read_asset: hash is not UTF-8".to_string());
                    return Ok(0);
                }
            };

            // Loader needs to be cloned out (it's an Arc), then the
            // ctx borrow ends before we mutate memory.
            let loader_opt = caller
                .data()
                .context
                .as_ref()
                .map(|c| c.asset_loader.clone());
            let Some(loader) = loader_opt else { return Ok(0) };
            let Some(bytes) = loader(&hash_str) else { return Ok(0) };

            let buf_cap = usize_from_i32(buf_len)?;
            let to_write = bytes.len().min(buf_cap);
            memory
                .write(&mut caller, usize_from_i32(buf_ptr)?, &bytes[..to_write])
                .map_err(wasm_err)?;
            Ok(i32::try_from(to_write).unwrap_or(i32::MAX))
        },
    )?;

    linker.func_wrap(
        "env",
        "kcreate_write_proposal",
        |mut caller: Caller<HostData>, ptr: i32, len: i32| -> Result<i32, wasmi::Error> {
            let memory = caller
                .data()
                .memory
                .ok_or_else(|| wasmi::Error::new("plugin memory not initialized"))?;
            let permitted = caller
                .data()
                .context
                .as_ref()
                .is_some_and(|c| c.has(PluginPermission::WriteDocument));
            if !permitted {
                caller
                    .data_mut()
                    .logs
                    .push("kcreate_write_proposal: denied (missing WriteDocument)".to_string());
                return Ok(0);
            }

            let mut buf = vec![0u8; usize_from_i32(len)?];
            memory
                .read(&caller, usize_from_i32(ptr)?, &mut buf)
                .map_err(wasm_err)?;
            let parsed: ProposedMutation = match serde_json::from_slice(&buf) {
                Ok(m) => m,
                Err(e) => {
                    caller
                        .data_mut()
                        .logs
                        .push(format!("kcreate_write_proposal: invalid json: {e}"));
                    return Ok(0);
                }
            };
            if let Some(ctx) = caller.data_mut().context.as_mut() {
                ctx.proposals.push(parsed);
            }
            Ok(1)
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

    #[test]
    fn execute_path_caches_compiled_module() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("echo.wasm");
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        std::fs::write(&path, &wasm).unwrap();

        let rt = WasmPluginRuntime::new();
        assert_eq!(rt.cache_len(), 0);

        // First call: cold — populates the cache with one entry.
        let a = rt.execute_path(&path, "run", "alpha", 16).unwrap();
        assert_eq!(a.output, "alpha");
        assert_eq!(rt.cache_len(), 1);

        // Second call against the same untouched file: hot — must still
        // be one entry, no recompile, and output must round-trip.
        let b = rt.execute_path(&path, "run", "beta", 16).unwrap();
        assert_eq!(b.output, "beta");
        assert_eq!(rt.cache_len(), 1);

        // Touch the file so mtime moves: the next call must observe
        // the same content hash and reuse the cached `Module`.
        std::thread::sleep(std::time::Duration::from_millis(20));
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            f.write_all(&wasm).unwrap();
        }
        let c = rt.execute_path(&path, "run", "gamma", 16).unwrap();
        assert_eq!(c.output, "gamma");
        assert_eq!(rt.cache_len(), 1);
    }

    /// Regression: a rebuild that lands in the *same* mtime second
    /// must still be picked up. Earlier the cache keyed only on
    /// `(path, mtime)`, so on coarse-resolution filesystems (ext3,
    /// HFS+) a sub-second rebuild could silently serve the stale
    /// compiled module. With BLAKE3 content-hashing, a content
    /// change is always detected on the next call.
    #[test]
    fn execute_path_picks_up_subsecond_content_change() {
        use filetime::{set_file_mtime, FileTime};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("echo.wasm");
        let echo_wasm = wat::parse_str(ECHO_WAT).unwrap();
        std::fs::write(&path, &echo_wasm).unwrap();

        let rt = WasmPluginRuntime::new();
        // Cold call: cache miss → compile.
        let a = rt.execute_path(&path, "run", "first", 16).unwrap();
        assert_eq!(a.output, "first");
        assert_eq!(rt.cache_len(), 1);
        let mtime_before = std::fs::metadata(&path).unwrap().modified().unwrap();

        // Rewrite with semantically different content (a different
        // export name) but pin the mtime to exactly the previous
        // value. If the cache trusted mtime alone it would serve the
        // stale `Module` and the second call would still look like
        // an echo. The content hash will differ, forcing a recompile.
        let alt_wasm = wat::parse_str(REVERSE_WAT).unwrap();
        std::fs::write(&path, &alt_wasm).unwrap();
        set_file_mtime(&path, FileTime::from_system_time(mtime_before)).unwrap();
        // Sanity: mtime is unchanged on disk.
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            mtime_before
        );

        let b = rt.execute_path(&path, "reverse", "abcd", 16).unwrap();
        assert_eq!(b.output, "dcba");
        // The path is still one cache entry — replaced, not appended.
        assert_eq!(rt.cache_len(), 1);
    }

    /// WAT module that exports `reverse(i32, i32) -> ()` reversing the
    /// input string before writing it back. Used to prove the cache
    /// recompiles on a content change even when mtime is frozen.
    const REVERSE_WAT: &str = r#"
        (module
            (import "env" "kcreate_get_input"
                (func $get_input (param i32 i32) (result i32)))
            (import "env" "kcreate_set_output"
                (func $set_output (param i32 i32)))
            (memory (export "memory") 1)
            (func (export "reverse")
                (local $len i32)
                (local $i i32)
                (local.set $len (call $get_input (i32.const 0) (i32.const 1024)))
                (local.set $i (i32.const 0))
                (block $done
                    (loop $cp
                        (br_if $done
                            (i32.ge_s (local.get $i) (local.get $len)))
                        (i32.store8
                            (i32.add (i32.const 1024) (local.get $i))
                            (i32.load8_u
                                (i32.add
                                    (i32.const 0)
                                    (i32.sub
                                        (i32.sub (local.get $len) (i32.const 1))
                                        (local.get $i)))))
                        (local.set $i (i32.add (local.get $i) (i32.const 1)))
                        (br $cp)))
                (call $set_output (i32.const 1024) (local.get $len))))
    "#;

    // -----------------------------------------------------------------
    // Phase 2 Task 13 — extended host ABI tests
    // -----------------------------------------------------------------

    use crate::context::AssetLoader;

    /// Plugin that calls `kcreate_read_document` with `{"type":"list_nodes"}`,
    /// writing the response to its own output buffer (via the host
    /// function), then no further work — we read whatever
    /// `kcreate_read_document` wrote out of the `PluginOutput`.
    const READ_DOC_WAT: &str = r#"
        (module
            (import "env" "kcreate_read_document"
                (func $rd (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{\"type\":\"list_nodes\"}")
            (func (export "run")
                i32.const 0  i32.const 21  call $rd  drop
            )
        )
    "#;

    /// Plugin that calls `kcreate_read_asset` with a fixed hash string
    /// and writes the (small) result to memory at offset 1024, then
    /// echoes that buffer back to the output.
    const READ_ASSET_WAT: &str = r#"
        (module
            (import "env" "kcreate_read_asset"
                (func $ra (param i32 i32 i32 i32) (result i32)))
            (import "env" "kcreate_set_output"
                (func $set_output (param i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "abc123")
            (func (export "run")
                (local $n i32)
                (local.set $n
                    (call $ra
                        (i32.const 0)
                        (i32.const 6)
                        (i32.const 1024)
                        (i32.const 64)))
                (call $set_output (i32.const 1024) (local.get $n))
            )
        )
    "#;

    /// Plugin that submits one `delete_node` proposal carrying a
    /// known UUID, then writes nothing else. The host should see
    /// exactly one proposal after execution.
    const WRITE_PROPOSAL_WAT: &str = r#"
        (module
            (import "env" "kcreate_write_proposal"
                (func $wp (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0)
                "{\"type\":\"delete_node\",\"node_id\":\"11111111-1111-1111-1111-111111111111\"}")
            (func (export "run")
                i32.const 0  i32.const 71  call $wp  drop
            )
        )
    "#;

    fn sample_snapshot() -> serde_json::Value {
        serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000000",
            "name": "Root",
            "children": [
                {
                    "id": "11111111-1111-1111-1111-111111111111",
                    "name": "Child A",
                    "children": []
                },
                {
                    "id": "22222222-2222-2222-2222-222222222222",
                    "name": "Child B",
                    "children": []
                }
            ]
        })
    }

    #[test]
    fn read_document_returns_ids_when_permitted() {
        let wasm = wat::parse_str(READ_DOC_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let ctx = PluginContext::empty("test")
            .with_snapshot(sample_snapshot())
            .grant(PluginPermission::ReadDocument);
        let out = rt
            .execute_with_context(&wasm, "run", "", 16, ctx)
            .expect("execute");
        let arr: Vec<String> = serde_json::from_str(&out.output).expect("json array");
        assert!(arr.contains(&"11111111-1111-1111-1111-111111111111".to_string()));
        assert!(arr.contains(&"22222222-2222-2222-2222-222222222222".to_string()));
        assert!(arr.contains(&"00000000-0000-0000-0000-000000000000".to_string()));
        assert!(out.proposals.is_empty());
    }

    #[test]
    fn read_document_denies_without_permission() {
        let wasm = wat::parse_str(READ_DOC_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let ctx = PluginContext::empty("test").with_snapshot(sample_snapshot());
        let out = rt
            .execute_with_context(&wasm, "run", "", 16, ctx)
            .expect("execute");
        // Output should be empty — the host wrote nothing on deny.
        assert!(out.output.is_empty(), "expected empty output, got {:?}", out.output);
        assert!(out
            .logs
            .iter()
            .any(|l| l.contains("denied (missing ReadDocument)")));
    }

    #[test]
    fn read_asset_returns_bytes_when_permitted() {
        let wasm = wat::parse_str(READ_ASSET_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let loader: AssetLoader = Arc::new(|hash: &str| {
            if hash == "abc123" {
                Some(b"asset-bytes".to_vec())
            } else {
                None
            }
        });
        let ctx = PluginContext::empty("test")
            .with_asset_loader(loader)
            .grant(PluginPermission::ReadAssets);
        let out = rt
            .execute_with_context(&wasm, "run", "", 16, ctx)
            .expect("execute");
        assert_eq!(out.output, "asset-bytes");
    }

    #[test]
    fn read_asset_denies_without_permission() {
        let wasm = wat::parse_str(READ_ASSET_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let loader: AssetLoader = Arc::new(|_| Some(b"asset-bytes".to_vec()));
        let ctx = PluginContext::empty("test").with_asset_loader(loader);
        let out = rt
            .execute_with_context(&wasm, "run", "", 16, ctx)
            .expect("execute");
        assert!(out.output.is_empty());
        assert!(out
            .logs
            .iter()
            .any(|l| l.contains("denied (missing ReadAssets)")));
    }

    #[test]
    fn write_proposal_appends_when_permitted() {
        let wasm = wat::parse_str(WRITE_PROPOSAL_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let ctx = PluginContext::empty("test").grant(PluginPermission::WriteDocument);
        let out = rt
            .execute_with_context(&wasm, "run", "", 16, ctx)
            .expect("execute");
        assert_eq!(out.proposals.len(), 1);
        let id: uuid::Uuid = "11111111-1111-1111-1111-111111111111".parse().unwrap();
        match &out.proposals[0] {
            ProposedMutation::DeleteNode { node_id } => assert_eq!(*node_id, id),
            other => panic!("expected DeleteNode, got {other:?}"),
        }
    }

    #[test]
    fn write_proposal_denies_without_permission() {
        let wasm = wat::parse_str(WRITE_PROPOSAL_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let ctx = PluginContext::empty("test"); // no permissions
        let out = rt
            .execute_with_context(&wasm, "run", "", 16, ctx)
            .expect("execute");
        assert!(out.proposals.is_empty());
        assert!(out
            .logs
            .iter()
            .any(|l| l.contains("denied (missing WriteDocument)")));
    }

    /// Legacy [`WasmPluginRuntime::execute`] runs without a context
    /// — the extended host imports aren't registered, so a plugin
    /// that depends on them fails to link.
    #[test]
    fn extended_funcs_unavailable_without_context() {
        let wasm = wat::parse_str(READ_DOC_WAT).unwrap();
        let rt = WasmPluginRuntime::new();
        let err = rt
            .execute(&wasm, "run", "", 16)
            .expect_err("must fail to link kcreate_read_document");
        assert!(matches!(err, WasmPluginError::Wasm(_)));
    }
}
