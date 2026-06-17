//! Rune plugin runtime.
//!
//! Plugins are `.rn` files in `~/.local/share/qbrsh/plugins/`. Each is compiled
//! into its own sandboxed VM (only the `qbrsh` API plus Rune's pure default
//! modules are available, with no filesystem, network, or process access). A
//! plugin's `main()` runs at load to register cold-event hooks; the browser
//! fires those hooks (page load, tab open, command) by name.
//!
//! Plugins run on the main thread under a per-invocation instruction budget, so
//! a runaway script is aborted rather than freezing the UI. Browser actions are
//! requested by sending messages on the mailbox; the API is fire-and-forget
//! (awaitable results that suspend the VM, such as reading a page value, are a
//! planned enhancement built on the same effect round-trip the core already uses
//! for JS results).
//!
//! Escape hatch: for CPU-heavy or untrusted automation that should not run
//! in-process, the intended path is an external program driving the browser over
//! an IPC/JSON-RPC control interface (a separate surface from these plugins),
//! rather than widening the in-process plugin API.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rune::runtime::budget;
use rune::termcolor::{ColorChoice, StandardStream};
use rune::{Context, Diagnostics, Module, Source, Sources, Vm};

use crate::core::command::{Command, OpenTarget};
use crate::core::msg::Msg;
use crate::core::runtime::Mailbox;

/// Maximum VM instructions per plugin invocation before it is aborted.
const BUDGET: usize = 1_000_000;

/// Event name → registered handler function names.
type Hooks = Arc<Mutex<HashMap<String, Vec<String>>>>;

struct LoadedPlugin {
    name: String,
    vm: Vm,
    hooks: Hooks,
}

/// Loads, runs, and fires hooks for Rune plugins.
pub struct PluginRuntime {
    dir: PathBuf,
    mailbox: Mailbox,
    plugins: Vec<LoadedPlugin>,
}

impl PluginRuntime {
    /// Create the runtime and load all plugins from `dir`.
    pub fn new(dir: PathBuf, mailbox: Mailbox) -> Self {
        let mut runtime = Self {
            dir,
            mailbox,
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

    /// Fire a cold-event hook on every plugin that registered for `event`.
    pub fn fire(&mut self, event: &str, arg: &str) {
        for plugin in &mut self.plugins {
            let handlers = plugin
                .hooks
                .lock()
                .map(|h| h.get(event).cloned().unwrap_or_default())
                .unwrap_or_default();
            for handler in handlers {
                let call = budget::with(BUDGET, || {
                    plugin.vm.call([handler.as_str()], (arg.to_string(),))
                })
                .call();
                if let Err(e) = call {
                    eprintln!("[qbrsh] plugin '{}' hook '{handler}' error: {e}", plugin.name);
                }
            }
        }
    }

    fn load_all(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rn") {
                match load_one(&path, &self.mailbox) {
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

/// Build the sandboxed `qbrsh` API module backed by `mailbox` and `hooks`.
fn build_module(mailbox: Mailbox, hooks: Hooks) -> Result<Module, rune::ContextError> {
    let mut module = Module::with_crate("qbrsh")?;

    let mb = mailbox.clone();
    module
        .function("command", move |s: String| {
            if let Ok(cmd) = Command::parse(&s) {
                mb.send(Msg::Command(cmd));
            }
        })
        .build()?;

    let mb = mailbox.clone();
    module
        .function("open", move |url: String| {
            mb.send(Msg::Command(Command::Open {
                target: OpenTarget::Current,
                input: url,
            }));
        })
        .build()?;

    let mb = mailbox.clone();
    module
        .function("message", move |text: String| {
            mb.send(Msg::PluginMessage(text));
        })
        .build()?;

    let mb = mailbox.clone();
    module
        .function("eval_js", move |script: String| {
            mb.send(Msg::PluginEval(script));
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
fn load_one(path: &Path, mailbox: &Mailbox) -> Result<LoadedPlugin, String> {
    let hooks: Hooks = Arc::new(Mutex::new(HashMap::new()));
    let module = build_module(mailbox.clone(), hooks.clone()).map_err(|e| e.to_string())?;

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
    let unit = result.map_err(|e| e.to_string())?;
    let mut vm = Vm::new(runtime, Arc::new(unit));

    // Run main() (if present) under budget to register hooks.
    let _ = budget::with(BUDGET, || vm.call(["main"], ())).call();

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin")
        .to_string();
    Ok(LoadedPlugin { name, vm, hooks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::msg::Msg;
    use crate::core::runtime::Mailbox;

    fn load_from_source(src: &str, mailbox: &Mailbox) -> LoadedPlugin {
        let hooks: Hooks = Arc::new(Mutex::new(HashMap::new()));
        let module = build_module(mailbox.clone(), hooks.clone()).unwrap();
        let mut context = Context::with_default_modules().unwrap();
        context.install(&module).unwrap();
        let runtime = Arc::new(context.runtime().unwrap());
        let mut sources = Sources::new();
        sources.insert(Source::memory(src).unwrap()).unwrap();
        let unit = rune::prepare(&mut sources)
            .with_context(&context)
            .build()
            .unwrap();
        let mut vm = Vm::new(runtime, Arc::new(unit));
        let _ = budget::with(BUDGET, || vm.call(["main"], ())).call();
        LoadedPlugin {
            name: "test".to_string(),
            vm,
            hooks,
        }
    }

    #[test]
    fn hook_registration_and_dispatch() {
        let (mailbox, rx) = Mailbox::channel();
        let plugin = load_from_source(
            r#"
            pub fn main() { qbrsh::on("page_load", "on_load"); }
            pub fn on_load(url) { qbrsh::open(url); }
            "#,
            &mailbox,
        );
        assert_eq!(
            plugin.hooks.lock().unwrap().get("page_load").map(|v| v.len()),
            Some(1)
        );

        let mut rt = PluginRuntime {
            dir: PathBuf::new(),
            mailbox: mailbox.clone(),
            plugins: vec![plugin],
        };
        rt.fire("page_load", "https://example.com");

        // The hook called qbrsh::open, which enqueued an Open command.
        match rx.try_recv() {
            Ok(Msg::Command(_)) => {}
            other => panic!("expected an Open command, got {other:?}"),
        }
    }

    #[test]
    fn runaway_plugin_is_budget_aborted() {
        let (mailbox, _rx) = Mailbox::channel();
        let plugin = load_from_source(
            r#"
            pub fn main() { qbrsh::on("page_load", "spin"); }
            pub fn spin(_url) { let n = 0; while true { n += 1; } }
            "#,
            &mailbox,
        );
        let mut rt = PluginRuntime {
            dir: PathBuf::new(),
            mailbox,
            plugins: vec![plugin],
        };
        // Must return (budget aborts the infinite loop) rather than hang.
        rt.fire("page_load", "x");
    }
}
