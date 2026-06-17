//! Rune plugin runtime.
//!
//! Plugins are `.rn` files in `~/.local/share/qbrsh/plugins/`. Each is compiled
//! into its own sandboxed unit (only the `qbrsh` API plus Rune's pure default
//! modules are available, with no filesystem, network, or process access). A
//! plugin's `main()` runs at load to register cold-event hooks; the browser
//! fires those hooks (page load, tab open, command) by name.
//!
//! Hooks run as async tasks on the glib main loop under a per-invocation
//! instruction budget, so a runaway script is aborted rather than freezing the
//! UI. The `qbrsh` API requests browser actions by sending messages; `eval_js`
//! is awaitable: it suspends the plugin VM, asks the browser to evaluate the
//! script in the active tab, and resumes with the result via the same effect
//! round-trip the core uses for JS results.
//!
//! Escape hatch: for CPU-heavy or untrusted automation that should not run
//! in-process, the intended path is an external program driving the browser over
//! an IPC/JSON-RPC control interface (a separate surface from these plugins),
//! rather than widening the in-process plugin API.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rune::runtime::{RuntimeContext, Unit, budget};
use rune::termcolor::{ColorChoice, StandardStream};
use rune::{Context, Diagnostics, Module, Source, Sources, Vm};

use crate::core::command::{Command, OpenTarget};
use crate::core::msg::Msg;
use crate::core::runtime::Mailbox;

/// Maximum VM instructions per plugin invocation before it is aborted.
const BUDGET: usize = 1_000_000;

/// Event name to registered handler function names.
type Hooks = Arc<Mutex<HashMap<String, Vec<String>>>>;

/// Shared state for plugin to browser communication.
struct Bridge {
    mailbox: Mailbox,
    /// In-flight awaited evaluations: request id to result sender.
    pending: Mutex<HashMap<u64, async_channel::Sender<String>>>,
    next_id: AtomicU64,
}

impl Bridge {
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

struct LoadedPlugin {
    name: String,
    runtime: Arc<RuntimeContext>,
    unit: Arc<Unit>,
    hooks: Hooks,
}

/// Loads, runs, and fires hooks for Rune plugins.
pub struct PluginRuntime {
    dir: PathBuf,
    bridge: Arc<Bridge>,
    plugins: Vec<LoadedPlugin>,
}

impl PluginRuntime {
    /// Create the runtime and load all plugins from `dir`.
    pub fn new(dir: PathBuf, mailbox: Mailbox) -> Self {
        let bridge = Arc::new(Bridge {
            mailbox,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        });
        let mut runtime = Self {
            dir,
            bridge,
            plugins: Vec::new(),
        };
        runtime.load_all();
        runtime
    }

    /// Number of currently-loaded plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }

    /// Recompile and reload all plugins from disk.
    pub fn reload(&mut self) {
        self.plugins.clear();
        self.load_all();
    }

    /// Fire a cold-event hook on every plugin registered for `event`. Each
    /// handler runs as an async task so it can await `eval_js`.
    pub fn fire(&self, event: &str, arg: &str) {
        for plugin in &self.plugins {
            let handlers = plugin
                .hooks
                .lock()
                .map(|h| h.get(event).cloned().unwrap_or_default())
                .unwrap_or_default();
            for handler in handlers {
                spawn_hook(
                    plugin.name.clone(),
                    plugin.runtime.clone(),
                    plugin.unit.clone(),
                    handler,
                    arg.to_string(),
                );
            }
        }
    }

    /// Deliver an awaited JS result back to the suspended plugin.
    pub fn resolve(&self, id: u64, result: String) {
        if let Ok(mut pending) = self.bridge.pending.lock()
            && let Some(tx) = pending.remove(&id)
        {
            let _ = tx.try_send(result);
        }
    }

    fn load_all(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rn") {
                match load_one(&path, &self.bridge) {
                    Ok(plugin) => {
                        eprintln!("[qbrsh] loaded plugin: {}", plugin.name);
                        self.plugins.push(plugin);
                    }
                    Err(e) => eprintln!("[qbrsh] plugin {} failed to load: {e}", path.display()),
                }
            }
        }
    }
}

/// Drive one hook handler to completion as an async task under budget.
fn spawn_hook(name: String, runtime: Arc<RuntimeContext>, unit: Arc<Unit>, handler: String, arg: String) {
    glib::MainContext::default().spawn_local(async move {
        let mut vm = Vm::new(runtime, unit);
        let outcome = match vm.execute([handler.as_str()], (arg,)) {
            Ok(mut execution) => budget::with(BUDGET, execution.async_complete())
                .await
                .into_result()
                .map(|_| ())
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = outcome {
            eprintln!("[qbrsh] plugin '{name}' hook '{handler}' error: {e}");
        }
    });
}

/// Build the sandboxed `qbrsh` API module backed by `bridge` and `hooks`.
fn build_module(bridge: Arc<Bridge>, hooks: Hooks) -> Result<Module, rune::ContextError> {
    let mut module = Module::with_crate("qbrsh")?;

    let b = bridge.clone();
    module
        .function("command", move |s: String| {
            if let Ok(cmd) = Command::parse(&s) {
                b.mailbox.send(Msg::Command(cmd));
            }
        })
        .build()?;

    let b = bridge.clone();
    module
        .function("open", move |url: String| {
            b.mailbox.send(Msg::Command(Command::Open {
                target: OpenTarget::Current,
                input: url,
            }));
        })
        .build()?;

    let b = bridge.clone();
    module
        .function("message", move |text: String| {
            b.mailbox.send(Msg::PluginMessage(text));
        })
        .build()?;

    // Awaitable: suspends the plugin until the browser returns the JS result.
    let b = bridge.clone();
    module
        .function("eval_js", move |script: String| {
            let bridge = b.clone();
            async move {
                let id = bridge.next_id();
                let (tx, rx) = async_channel::bounded::<String>(1);
                if let Ok(mut pending) = bridge.pending.lock() {
                    pending.insert(id, tx);
                }
                bridge.mailbox.send(Msg::PluginEvalRequest { id, script });
                rx.recv().await.unwrap_or_default()
            }
        })
        .build()?;

    module
        .function("on", move |event: String, handler: String| {
            if let Ok(mut h) = hooks.lock() {
                h.entry(event).or_default().push(handler);
            }
        })
        .build()?;

    Ok(module)
}

/// Compile and initialize a single plugin file.
fn load_one(path: &Path, bridge: &Arc<Bridge>) -> Result<LoadedPlugin, String> {
    let hooks: Hooks = Arc::new(Mutex::new(HashMap::new()));
    let module = build_module(bridge.clone(), hooks.clone()).map_err(|e| e.to_string())?;

    let mut context = Context::with_default_modules().map_err(|e| e.to_string())?;
    context.install(&module).map_err(|e| e.to_string())?;
    let runtime = Arc::new(context.runtime().map_err(|e| e.to_string())?);

    let mut sources = Sources::new();
    let source = Source::from_path(path).map_err(|e| e.to_string())?;
    sources.insert(source).map_err(|e| e.to_string())?;

    let mut diagnostics = Diagnostics::new();
    let result = rune::prepare(&mut sources)
        .with_context(&context)
        .with_diagnostics(&mut diagnostics)
        .build();
    if !diagnostics.is_empty() {
        let mut writer = StandardStream::stderr(ColorChoice::Auto);
        let _ = diagnostics.emit(&mut writer, &sources);
    }
    let unit = Arc::new(result.map_err(|e| e.to_string())?);

    // Run main() (if present) under budget to register hooks. main() is for
    // registration and should be synchronous (it must not await).
    let mut vm = Vm::new(runtime.clone(), unit.clone());
    let _ = budget::with(BUDGET, || vm.call(["main"], ())).call();

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin")
        .to_string();
    Ok(LoadedPlugin {
        name,
        runtime,
        unit,
        hooks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime::Mailbox;

    fn bridge() -> Arc<Bridge> {
        let (mailbox, _rx) = Mailbox::channel();
        Arc::new(Bridge {
            mailbox,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    fn load_source(src: &str, bridge: &Arc<Bridge>) -> LoadedPlugin {
        let hooks: Hooks = Arc::new(Mutex::new(HashMap::new()));
        let module = build_module(bridge.clone(), hooks.clone()).unwrap();
        let mut context = Context::with_default_modules().unwrap();
        context.install(&module).unwrap();
        let runtime = Arc::new(context.runtime().unwrap());
        let mut sources = Sources::new();
        sources.insert(Source::memory(src).unwrap()).unwrap();
        let unit = Arc::new(
            rune::prepare(&mut sources)
                .with_context(&context)
                .build()
                .unwrap(),
        );
        let mut vm = Vm::new(runtime.clone(), unit.clone());
        let _ = budget::with(BUDGET, || vm.call(["main"], ())).call();
        LoadedPlugin {
            name: "test".to_string(),
            runtime,
            unit,
            hooks,
        }
    }

    #[test]
    fn main_registers_hooks() {
        let plugin = load_source(
            r#"pub fn main() { qbrsh::on("page_load", "on_load"); }
               pub fn on_load(url) { qbrsh::message(url); }"#,
            &bridge(),
        );
        let hooks = plugin.hooks.lock().unwrap();
        assert_eq!(hooks.get("page_load").map(|v| v.len()), Some(1));
    }

    #[test]
    fn runaway_main_is_budget_aborted() {
        // load_source runs main() under budget; an infinite loop must abort and
        // return rather than hang.
        let _plugin = load_source(
            r#"pub fn main() { let n = 0; while true { n += 1; } }"#,
            &bridge(),
        );
    }
}
