# Plugins

qbrsh is extended with plugins written in [Rune](https://rune-rs.github.io), a
small embeddable scripting language with Rust-like syntax. Plugins are
sandboxed, run under an instruction budget, and react to browser events.

## Where plugins live

Drop `.rn` files into `~/.local/share/qbrsh/plugins/`. Each file is compiled
into its own sandboxed unit; its `main()` runs at load to register hooks. Reload
all plugins without restarting with `:plugin-reload`.

A failed plugin is reported and skipped; it does not stop other plugins or the
browser from loading.

## A first plugin

```rust
pub fn main() {
    qbrsh::on("page_load", "on_page_load");
}

pub async fn on_page_load(url) {
    let title = qbrsh::eval_js("document.title").await;
    qbrsh::message(`loaded "${title}" (${url})`);
}
```

`main` registers a hook by handler name. When a page finishes loading, qbrsh
calls `on_page_load` with the URL. The handler reads the page title with an
awaited `eval_js` and shows a status-bar message.

## What plugins can do

- React to cold events: `page_load`, `tab_open`, and `command`.
- Run commands and open URLs.
- Read values from the page with `eval_js(...).await`.
- Show status-bar messages.

See the full [plugin API](/reference/plugin-api).

## Safety

Plugins see only the `qbrsh` API plus Rune's pure standard modules. There is no
filesystem, network, or process access, and each hook invocation is capped by an
instruction budget, so a runaway script is aborted rather than freezing the UI.

For heavy or untrusted automation, use the
[IPC control interface](/guide/automation) from an external process instead of
widening what plugins can do.
